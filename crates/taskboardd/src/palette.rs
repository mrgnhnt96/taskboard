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
    (0..chars.len()).find_map(|start| {
        let ok = want.len() <= chars.len() - start && want.iter().enumerate().all(|(k, w)| low(chars[start + k].1) == *w);
        ok.then(|| {
            let end = chars.get(start + want.len()).map(|(b, _)| *b).unwrap_or(hay.len());
            chars[start].0..end
        })
    })
}

/// One char in lower case, the same way for the query and the text it's matched against: a
/// whole-string lower case can differ (a final sigma, a dotted I) and then never match.
fn low(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The query as lower-case words.
pub fn words(q: &str) -> Vec<String> {
    q.split_whitespace().map(|w| w.chars().map(low).collect()).collect()
}

/// Whether `w` names the ref `r` (both lower case): its whole (`t332`), or its number alone (`332`).
/// `Some(true)` for all of it, `Some(false)` for its start, `None` when it doesn't.
fn ref_match(r: &str, w: &str) -> Option<bool> {
    let num = r.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    let bare = !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()) && num.len() < r.len();
    if r.is_empty() {
        None
    } else if r == w || (bare && num == w) {
        Some(true)
    } else if r.starts_with(w) || (bare && num.starts_with(w)) {
        Some(false)
    } else {
        None
    }
}

/// How well an object matches: `None` when a word is found nowhere, else lower is better —
/// its ref (with or without its letter), then its title's start, a word in its title, anywhere in
/// its title, then its other text.
/// The highlights are each word's first place in the title.
pub fn score(r: &str, title: &str, other: &[&str], words: &[String]) -> Option<(u8, Vec<Range<usize>>)> {
    let r: String = r.chars().map(low).collect();
    let mut best = u8::MAX;
    let mut hl = Vec::new();
    for w in words {
        let in_title = find_ci(title, w);
        let rank = if let Some(whole) = ref_match(&r, w) {
            if whole { 0 } else { 1 }
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
///
/// With `goal` (`&goal=<id>`, ⌘K on a goal's page) only that goal's tasks and issues, and a task's
/// words are its title, detail and summary: the goal's name would match every one of them. Its
/// tasks then carry their `detail` so the app can mark the same matches.
///
/// Attachments too, once something is typed: the goal's and its tasks' (else every one), by their
/// title, then their summary, kind and link; each says whose it is (`task` or `goal`, a ref).
pub fn find(app: &App, q: &str, limit: usize, goal_id: Option<i64>) -> Result<Value> {
    let w = words(q);
    let in_goal = |id: Option<i64>| goal_id.is_none() || id == goal_id;
    let goals: HashMap<i64, String> = app.db.q("SELECT id, name FROM goals", p![])?.iter().map(|g| (g.id(), g.st("name"))).collect();
    let goal = |id: Option<i64>| id.and_then(|g| goals.get(&g)).map(String::as_str).unwrap_or("");

    let mut tasks = vec![];
    for t in app.db.q("SELECT * FROM tasks", p![])? {
        if !in_goal(t.i("goal_id")) {
            continue;
        }
        let open = t.s("status") != Some("done");
        let sc = if w.is_empty() {
            if !open {
                continue;
            }
            0
        } else {
            let s = |k| t.s(k).unwrap_or("");
            let other = if goal_id.is_some() { [s("detail"), s("summary")] } else { [s("project"), goal(t.i("goal_id"))] };
            match score(&rf("task", t.id()), s("title"), &other, &w) {
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
            if !in_goal(b.i("goal_id")) {
                continue;
            }
            let other = [b.s("project").unwrap_or(""), goal(b.i("goal_id")), b.s("area").unwrap_or(""), b.s("detail").unwrap_or(""), b.s("said").unwrap_or("")];
            if let Some((sc, _)) = score(&rf("issue", b.id()), b.s("title").unwrap_or(""), &other, &w) {
                issues.push(((sc, issue_closed(&b) as u8, -b.id()), b));
            }
        }
    }
    let (issues, issues_total) = best(issues, limit);

    let mut atts = vec![];
    if !w.is_empty() {
        let goal_of: HashMap<i64, Option<i64>> = app.db.q("SELECT id, goal_id FROM tasks", p![])?.iter().map(|t| (t.id(), t.i("goal_id"))).collect();
        for a in app.db.q("SELECT * FROM attachments WHERE removed_at IS NULL", p![])? {
            let task = a.i("task_id");
            if !in_goal(task.map(|t| goal_of.get(&t).copied().flatten()).unwrap_or(a.i("goal_id"))) {
                continue;
            }
            let s = |k| a.s(k).unwrap_or("");
            if let Some((sc, _)) = score("", s("title"), &[s("summary"), s("kind"), s("url")], &w) {
                atts.push(((sc, 0, -a.id()), a));
            }
        }
    }
    let (atts, atts_total) = best(atts, limit);
    let atts: Vec<Value> = atts
        .iter()
        .map(|a| {
            json!({"id": a.id(), "title": a.v("title"), "summary": a.v("summary"), "kind": a.v("kind"), "url": a.v("url"),
                   "task": rf_opt("task", a.i("task_id")), "goal": rf_opt("goal", a.i("goal_id"))})
        })
        .collect();

    let mut cards_t = vec![];
    for t in &tasks {
        let mut c = board::task_card(app, t)?;
        if goal_id.is_some() {
            c["detail"] = t.v("detail");
        }
        cards_t.push(c);
    }
    let mut cards = vec![];
    for b in &issues {
        let mut d = board::issue_card(app, b)?;
        d["said"] = b.v("said");
        d["detail"] = b.v("detail");
        cards.push(d);
    }
    Ok(json!({"tasks": cards_t, "tasks_total": tasks_total, "issues": cards, "issues_total": issues_total, "attachments": atts, "attachments_total": atts_total}))
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
        search_in(app, q, limit, None)
    }

    fn search_in(app: &App, q: &str, limit: &str, goal: Option<i64>) -> Value {
        let mut q: crate::api::Query = [("palette", "1"), ("q", q), ("limit", limit)].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        if let Some(g) = goal {
            q.insert("goal".into(), g.to_string());
        }
        crate::api::dispatch(app, "GET", "/search", &q, &json!({})).unwrap()
    }

    fn refs(v: &Value, k: &str) -> Vec<String> {
        v[k].as_array().unwrap().iter().map(|c| c["ref"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn a_bare_number_finds_its_ref() {
        let w = words("332");
        assert_eq!(score("T332", "x", &[], &w).unwrap().0, 0);
        assert_eq!(score("B3320", "x", &[], &w).unwrap().0, 1);
        assert_eq!(score("T1332", "x", &[], &w), None, "a number inside the ref isn't it");
        assert_eq!(score("", "x", &["332"], &w).unwrap().0, 5, "no ref: other text");
        assert_eq!(score("T9", "x", &[], &words("t")).unwrap().0, 1, "a letter alone still starts a ref");
    }

    #[test]
    fn the_query_and_the_text_lower_case_alike() {
        assert!(score("T1", "ΟΔΟΣ", &[], &words("ΟΔΟΣ")).is_some(), "a final sigma");
        assert!(score("T1", "İstanbul", &[], &words("İst")).is_some(), "a dotted I");
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
        assert_eq!(refs(&search(&app, &other.to_string(), "6"), "tasks")[0], format!("T{other}"), "its number alone too");
        let none = search(&app, "", "500");
        assert_eq!(none["tasks_total"], 202, "nothing typed: the open tasks");
        assert_eq!(refs(&none, "tasks")[0], format!("T{needs}"), "the one that needs you first");
        assert_eq!(none["issues_total"], 0);
    }

    #[test]
    fn a_goal_keeps_its_own_and_finds_words_in_their_detail() {
        let (_d, app) = board();
        let g = app.db.insert("goals", crate::fields!["name" => "Passkeys", "project" => "webapp"]).unwrap();
        let other = app.db.insert("goals", crate::fields!["name" => "Billing", "project" => "webapp"]).unwrap();
        let mine = app.db.insert("tasks", crate::fields!["title" => "Settings page", "detail" => "List each passkey with a rename", "project" => "webapp", "status" => "queued", "goal_id" => g]).unwrap();
        let named = app.db.insert("tasks", crate::fields!["title" => "Rename flow", "project" => "webapp", "status" => "done", "summary" => "Shipped behind a flag", "goal_id" => g]).unwrap();
        app.db.insert("tasks", crate::fields!["title" => "Rename the invoice", "project" => "webapp", "status" => "queued", "goal_id" => other]).unwrap();
        app.db.insert("tasks", crate::fields!["title" => "Rename loose", "project" => "webapp", "status" => "queued"]).unwrap();
        let b = app.db.insert("issues", crate::fields!["project" => "webapp", "kind" => "bug", "title" => "Rename loses focus", "state" => "open", "goal_id" => g]).unwrap();
        app.db.insert("issues", crate::fields!["project" => "webapp", "kind" => "bug", "title" => "Rename the plan", "state" => "open"]).unwrap();

        let v = search_in(&app, "rename", "6", Some(g));
        // The title match first, then the one found in its detail; nothing from another goal.
        assert_eq!(refs(&v, "tasks"), [format!("T{named}"), format!("T{mine}")]);
        assert_eq!(v["tasks_total"], 2);
        assert_eq!(v["tasks"][1]["detail"], "List each passkey with a rename");
        assert_eq!(refs(&v, "issues"), [format!("B{b}")]);
        assert_eq!(refs(&search_in(&app, "flag", "6", Some(g)), "tasks"), [format!("T{named}")], "its summary too");
        assert_eq!(search_in(&app, "passkeys", "6", Some(g))["tasks_total"], 0, "the goal's own name matches none");
        assert_eq!(search_in(&app, "", "6", Some(g))["tasks_total"], 1, "nothing typed: its open tasks");
        assert_eq!(search(&app, "rename", "6")["tasks_total"], 3, "unscoped: every goal, no detail");

        let on_goal = |task: Option<i64>, goal: Option<i64>, title: &str, summary: &str, url: &str| {
            app.db.insert("attachments", crate::fields!["task_id" => task, "goal_id" => goal, "kind" => "design", "title" => title, "summary" => summary, "url" => url]).unwrap()
        };
        let a1 = on_goal(None, Some(g), "Rename mockups", "", "https://example.com/a");
        let a2 = on_goal(Some(mine), None, "Settings spec", "How rename works", "/tmp/spec.md");
        on_goal(None, Some(other), "Rename invoices", "", "https://example.com/b");
        let gone = on_goal(None, Some(g), "Rename old", "", "https://example.com/c");
        app.db.x("UPDATE attachments SET removed_at = 'x' WHERE id = ?", crate::p![gone]).unwrap();
        let v = search_in(&app, "rename", "6", Some(g));
        let ids: Vec<i64> = v["attachments"].as_array().unwrap().iter().map(|a| a["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, [a1, a2], "the goal's and its tasks', title matches first; not another goal's or a removed one");
        assert_eq!(v["attachments"][1]["task"], format!("T{mine}"));
        assert_eq!(v["attachments"][0]["goal"], format!("G{g}"));
        assert_eq!(search_in(&app, "spec.md", "6", Some(g))["attachments_total"], 1, "its link too");
        assert_eq!(search(&app, "rename", "6")["attachments_total"], 3, "unscoped: every one still attached");
        assert_eq!(search_in(&app, "", "6", Some(g))["attachments_total"], 0, "none until something's typed");
    }
}
