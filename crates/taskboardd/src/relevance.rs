//! Which lessons fit a piece of work. Two things are kept for it, in the board's own database:
//!
//! - `task_paths`: the files each task's PR changed, read with git from the PR's base and head as
//!   the PR moves (`tick`), so a task's code areas outlast its branch.
//! - `search`: an FTS5 index of lessons, goal notes, task titles and details and backlog issues,
//!   kept in step by triggers. `find` searches it for `GET /search` (`tb search`).
//!
//! A lesson scores on its own text matching the work, on the tasks and goals it was learned from
//! (its evidence) matching it, and on how far the files its evidence changed overlap the files the
//! work is likely to touch: paths the work names, and the files changed by its goal's other tasks
//! and by the past tasks most like it.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::app::App;
use crate::p;
use crate::util::*;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS task_paths(task_id INT NOT NULL, path TEXT NOT NULL, PRIMARY KEY(task_id, path)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS task_paths_path ON task_paths(path);
CREATE TABLE IF NOT EXISTS task_path_reads(task_id INTEGER PRIMARY KEY, head TEXT, found INT DEFAULT 0, at TEXT);

CREATE VIRTUAL TABLE IF NOT EXISTS search USING fts5(
  text, kind UNINDEXED, rid UNINDEXED, goal UNINDEXED, project UNINDEXED, tokenize = 'porter unicode61');

CREATE TRIGGER IF NOT EXISTS search_lesson_ai AFTER INSERT ON lessons BEGIN
  INSERT INTO search(rowid, text, kind, rid, goal, project) VALUES (new.id * 4 + 1, new.text, 'lesson', new.id, NULL, new.project);
END;
CREATE TRIGGER IF NOT EXISTS search_lesson_au AFTER UPDATE OF text, project ON lessons BEGIN
  DELETE FROM search WHERE rowid = old.id * 4 + 1;
  INSERT INTO search(rowid, text, kind, rid, goal, project) VALUES (new.id * 4 + 1, new.text, 'lesson', new.id, NULL, new.project);
END;
CREATE TRIGGER IF NOT EXISTS search_lesson_ad AFTER DELETE ON lessons BEGIN
  DELETE FROM search WHERE rowid = old.id * 4 + 1;
END;

CREATE TRIGGER IF NOT EXISTS search_note_ai AFTER INSERT ON goal_notes BEGIN
  INSERT INTO search(rowid, text, kind, rid, goal, project)
  VALUES (new.id * 4 + 2, COALESCE(new.text, ''), 'note', new.id, new.goal_id, (SELECT project FROM goals WHERE id = new.goal_id));
END;
CREATE TRIGGER IF NOT EXISTS search_note_au AFTER UPDATE OF text, goal_id ON goal_notes BEGIN
  DELETE FROM search WHERE rowid = old.id * 4 + 2;
  INSERT INTO search(rowid, text, kind, rid, goal, project)
  VALUES (new.id * 4 + 2, COALESCE(new.text, ''), 'note', new.id, new.goal_id, (SELECT project FROM goals WHERE id = new.goal_id));
END;
CREATE TRIGGER IF NOT EXISTS search_note_ad AFTER DELETE ON goal_notes BEGIN
  DELETE FROM search WHERE rowid = old.id * 4 + 2;
END;

CREATE TRIGGER IF NOT EXISTS search_task_ai AFTER INSERT ON tasks BEGIN
  INSERT INTO search(rowid, text, kind, rid, goal, project)
  VALUES (new.id * 4 + 3, COALESCE(new.title, '') || char(10) || COALESCE(new.detail, ''), 'task', new.id, new.goal_id, new.project);
END;
CREATE TRIGGER IF NOT EXISTS search_task_au AFTER UPDATE OF title, detail, project, goal_id ON tasks BEGIN
  DELETE FROM search WHERE rowid = old.id * 4 + 3;
  INSERT INTO search(rowid, text, kind, rid, goal, project)
  VALUES (new.id * 4 + 3, COALESCE(new.title, '') || char(10) || COALESCE(new.detail, ''), 'task', new.id, new.goal_id, new.project);
END;
CREATE TRIGGER IF NOT EXISTS search_task_ad AFTER DELETE ON tasks BEGIN
  DELETE FROM search WHERE rowid = old.id * 4 + 3;
END;

CREATE TRIGGER IF NOT EXISTS search_issue_ai AFTER INSERT ON issues BEGIN
  INSERT INTO search(rowid, text, kind, rid, goal, project)
  VALUES (new.id * 4, COALESCE(new.title, '') || char(10) || COALESCE(new.detail, '') || char(10) || COALESCE(new.said, ''),
          'issue', new.id, new.goal_id, new.project);
END;
CREATE TRIGGER IF NOT EXISTS search_issue_au AFTER UPDATE OF title, detail, said, project, goal_id ON issues BEGIN
  DELETE FROM search WHERE rowid = old.id * 4;
  INSERT INTO search(rowid, text, kind, rid, goal, project)
  VALUES (new.id * 4, COALESCE(new.title, '') || char(10) || COALESCE(new.detail, '') || char(10) || COALESCE(new.said, ''),
          'issue', new.id, new.goal_id, new.project);
END;
CREATE TRIGGER IF NOT EXISTS search_issue_ad AFTER DELETE ON issues BEGIN
  DELETE FROM search WHERE rowid = old.id * 4;
END;

CREATE TRIGGER IF NOT EXISTS search_idea_ai AFTER INSERT ON ideas BEGIN
  INSERT INTO search(rowid, text, kind, rid, goal, project)
  VALUES (-new.id, COALESCE(new.title, '') || char(10) || COALESCE(new.detail, ''), 'idea', new.id, new.goal_id, new.project);
END;
CREATE TRIGGER IF NOT EXISTS search_idea_au AFTER UPDATE OF title, detail, project, goal_id ON ideas BEGIN
  DELETE FROM search WHERE rowid = -old.id;
  INSERT INTO search(rowid, text, kind, rid, goal, project)
  VALUES (-new.id, COALESCE(new.title, '') || char(10) || COALESCE(new.detail, ''), 'idea', new.id, new.goal_id, new.project);
END;
CREATE TRIGGER IF NOT EXISTS search_idea_ad AFTER DELETE ON ideas BEGIN
  DELETE FROM search WHERE rowid = -old.id;
END;
"#;

/// Fills the index once, for a board made before it existed (the triggers keep it after that).
pub const BACKFILL: &str = r#"
CREATE TEMP TABLE IF NOT EXISTS search_backfill AS SELECT NOT EXISTS (SELECT 1 FROM settings WHERE key = 'search_indexed') AS due;
INSERT INTO search(rowid, text, kind, rid, goal, project)
  SELECT id * 4 + 1, text, 'lesson', id, NULL, project FROM lessons WHERE (SELECT due FROM search_backfill);
INSERT INTO search(rowid, text, kind, rid, goal, project)
  SELECT n.id * 4 + 2, COALESCE(n.text, ''), 'note', n.id, n.goal_id, g.project FROM goal_notes n LEFT JOIN goals g ON g.id = n.goal_id
  WHERE (SELECT due FROM search_backfill);
INSERT INTO search(rowid, text, kind, rid, goal, project)
  SELECT id * 4 + 3, COALESCE(title, '') || char(10) || COALESCE(detail, ''), 'task', id, goal_id, project FROM tasks
  WHERE (SELECT due FROM search_backfill);
INSERT OR IGNORE INTO settings(key, value) VALUES ('search_indexed', datetime('now'));
DROP TABLE search_backfill;

CREATE TEMP TABLE IF NOT EXISTS search_backfill AS SELECT NOT EXISTS (SELECT 1 FROM settings WHERE key = 'search_issues_indexed') AS due;
DELETE FROM search WHERE kind = 'issue' AND (SELECT due FROM search_backfill);
INSERT INTO search(rowid, text, kind, rid, goal, project)
  SELECT id * 4, COALESCE(title, '') || char(10) || COALESCE(detail, '') || char(10) || COALESCE(said, ''), 'issue', id, goal_id, project
  FROM issues WHERE (SELECT due FROM search_backfill);
INSERT OR IGNORE INTO settings(key, value) VALUES ('search_issues_indexed', datetime('now'));
DROP TABLE search_backfill;

CREATE TEMP TABLE IF NOT EXISTS search_backfill AS SELECT NOT EXISTS (SELECT 1 FROM settings WHERE key = 'search_ideas_indexed') AS due;
DELETE FROM search WHERE kind = 'idea' AND (SELECT due FROM search_backfill);
INSERT INTO search(rowid, text, kind, rid, goal, project)
  SELECT -id, COALESCE(title, '') || char(10) || COALESCE(detail, ''), 'idea', id, goal_id, project
  FROM ideas WHERE (SELECT due FROM search_backfill);
INSERT OR IGNORE INTO settings(key, value) VALUES ('search_ideas_indexed', datetime('now'));
DROP TABLE search_backfill;
"#;

/// PRs read for their files on one tick, and how long before a read that found nothing is tried again.
const READS_PER_TICK: i64 = 3;
const RETRY_SECS: f64 = 86_400.0;
/// At most this many files are kept for one task.
const PATHS_MAX: usize = 200;

/// The work a lesson is picked for: its project, words, and (for a task) the task itself.
pub struct Work<'a> {
    pub project: Option<&'a str>,
    pub text: String,
    pub task: Option<&'a Row>,
}

/// A lesson's fit for the work, and what made it fit.
pub struct Scored {
    pub lesson: Row,
    pub score: f64,
    pub why: Vec<String>,
}

// ---- files each task changed ----

/// The files a task's PR changed, as recorded (empty until its PR has been read).
pub fn paths_of(app: &App, task_id: i64) -> Result<Vec<String>> {
    Ok(app.db.q("SELECT path FROM task_paths WHERE task_id = ? ORDER BY path", p![task_id])?.iter().map(|r| r.st("path")).collect())
}

/// Records the files a task changed, in place of what was there.
pub fn record(app: &App, task_id: i64, head: &str, paths: &[String]) -> Result<()> {
    app.db.tx(|| {
        if !paths.is_empty() {
            app.db.x("DELETE FROM task_paths WHERE task_id = ?", p![task_id])?;
            for path in paths.iter().take(PATHS_MAX) {
                app.db.x("INSERT OR IGNORE INTO task_paths(task_id, path) VALUES (?, ?)", p![task_id, path])?;
            }
        }
        app.db.x(
            "INSERT INTO task_path_reads(task_id, head, found, at) VALUES (?, ?, ?, ?)
             ON CONFLICT(task_id) DO UPDATE SET head = excluded.head, found = excluded.found, at = excluded.at",
            p![task_id, head, !paths.is_empty(), now_iso()],
        )?;
        Ok(())
    })
}

/// The files the PR in `rec` changes: base...head, or for a PR already merged with a merge commit
/// (head then sits in the base, so that diff is empty), the merge's first parent...head.
fn changed(repo: &str, rec: &Value) -> Vec<String> {
    let files = crate::picker::changed_files(repo, rec);
    if !files.is_empty() {
        return files;
    }
    let (base, head) = (rec["base_head"].as_str().unwrap_or(""), rec["head"].as_str().unwrap_or(""));
    if base.is_empty() || head.is_empty() {
        return vec![];
    }
    let merge = crate::picker::git(repo, &["rev-list", "--merges", "--ancestry-path", "--reverse", &format!("{head}..{base}")])
        .and_then(|o| o.lines().next().map(|l| l.trim().to_string()))
        .filter(|m| !m.is_empty());
    let Some(m) = merge else { return vec![] };
    crate::picker::git(repo, &["diff", "--name-only", &format!("{m}^1...{head}")])
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .take(PATHS_MAX)
        .collect()
}

/// Reads the files of a few PRs whose head moved since they were last read (or that were never
/// read). Runs git, so it's called outside a transaction.
pub fn tick(app: &App) -> Result<()> {
    let retry = iso(now_ts() - RETRY_SECS);
    let rows = app.db.q(
        "SELECT t.id, t.project, t.repo_path, t.pr_flow FROM tasks t LEFT JOIN task_path_reads r ON r.task_id = t.id
         WHERE json_valid(t.pr_flow) AND json_extract(t.pr_flow, '$.rec.head') IS NOT NULL
           AND (r.task_id IS NULL OR r.head IS NOT json_extract(t.pr_flow, '$.rec.head') OR (r.found = 0 AND r.at < ?))
         ORDER BY t.id DESC LIMIT ?",
        p![retry, READS_PER_TICK],
    )?;
    for t in rows {
        let rec = jloads_obj(t.s("pr_flow")).get("rec").cloned().unwrap_or(Value::Null);
        let head = rec["head"].as_str().unwrap_or("").to_string();
        let repo = crate::projects::project_path(app, t.s("project"))?.or_else(|| t.s("repo_path").map(|s| s.to_string()));
        let files = match repo.filter(|r| std::path::Path::new(r).is_dir()) {
            Some(r) => changed(&r, &rec),
            None => vec![],
        };
        record(app, t.id(), &head, &files)?;
    }
    Ok(())
}

// ---- text search ----

const STOP: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "are", "was", "were", "but", "not", "its", "it's", "you",
    "your", "they", "them", "then", "than", "when", "what", "which", "who", "how", "all", "any", "can", "each", "has",
    "have", "had", "out", "use", "one", "our", "also", "only", "just", "more", "most", "some", "such", "there", "their",
    "these", "those", "will", "would", "should", "could", "does", "did", "done", "make", "made", "task", "tasks", "goal",
    "goals", "here", "over", "after", "before", "about", "else", "instead", "same", "own", "new", "now",
];

/// The text's words worth matching, lowercased, each once.
fn words(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|w| w.to_lowercase())
        .filter(|w| w.chars().count() >= 3 && !w.chars().all(|c| c.is_ascii_digit()) && !STOP.contains(&w.as_str()))
        .filter(|w| seen.insert(w.clone()))
        .take(40)
        .collect()
}

/// An FTS5 query matching any of the text's words, or None when it has none worth matching.
pub fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = words(text).iter().map(|w| format!("\"{w}\"")).collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

/// One search hit: what it is (lesson, note, task), its id, its goal, and how well it matched (0 to 1).
struct Hit {
    kind: String,
    rid: i64,
    goal: Option<i64>,
    weight: f64,
}

/// The index's best matches for the text in a project (and the board's global lessons).
fn hits(app: &App, project: Option<&str>, text: &str) -> Result<Vec<Hit>> {
    let Some(q) = fts_query(text) else { return Ok(vec![]) };
    let rows = app.db.q(
        "SELECT kind, rid, goal, bm25(search) AS rank FROM search
         WHERE search MATCH ? AND kind NOT IN ('issue', 'idea') AND (project IS ? OR project IS NULL)
         ORDER BY rank LIMIT 80",
        p![q, project],
    )?;
    // bm25 is negative, more so for a better match.
    let best = rows.iter().filter_map(|r| r.f("rank")).fold(0.0_f64, |a, b| a.max(-b));
    Ok(rows
        .iter()
        .map(|r| Hit {
            kind: r.st("kind"),
            rid: r.i0("rid"),
            goal: r.i("goal"),
            weight: if best > 0.0 { (-r.f("rank").unwrap_or(0.0)).max(0.0) / best } else { 0.0 },
        })
        .collect())
}

// ---- board search ----

/// What `GET /search` looks for: the words, in a project (None: every project), of one kind (task,
/// issue, goal, note or idea; None: all of them), and whether only what's still open counts.
pub struct Find<'a> {
    pub text: &'a str,
    pub project: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub open: bool,
    pub limit: usize,
}

/// The tasks, backlog issues, goals, goal notes and ideas that match the words, best first, as cards with
/// their state.
pub fn find(app: &App, f: &Find) -> Result<Vec<Value>> {
    let Some(q) = fts_query(f.text) else { return Ok(vec![]) };
    let mut found: Vec<(f64, String, i64)> = vec![];
    if f.kind != Some("goal") {
        let rows = app.db.q(
            "SELECT kind, rid, bm25(search) AS rank FROM search
             WHERE search MATCH ? AND kind IN ('task', 'issue', 'note', 'idea') AND (? IS NULL OR kind = ?) AND (? IS NULL OR project IS ?)
             ORDER BY rank LIMIT 500",
            p![q, f.kind, f.kind, f.project, f.project],
        )?;
        // bm25 is negative, more so for a better match.
        let best = rows.iter().filter_map(|r| r.f("rank")).fold(0.0_f64, |a, b| a.max(-b));
        for r in &rows {
            let w = if best > 0.0 { (-r.f("rank").unwrap_or(0.0)).max(0.0) / best } else { 0.0 };
            found.push((w, r.st("kind"), r.i0("rid")));
        }
    }
    if f.kind.is_none() || f.kind == Some("goal") {
        // Goals aren't in the index: a goal scores the share of the words its name, outcome and summary hold.
        let ws = words(f.text);
        for g in app.db.q("SELECT id, name, outcome, tldr FROM goals WHERE ? IS NULL OR project IS ?", p![f.project, f.project])? {
            let hay = format!("{}\n{}\n{}", g.st("name"), g.st("outcome"), g.st("tldr")).to_lowercase();
            let n = ws.iter().filter(|w| hay.contains(w.as_str())).count();
            if n > 0 {
                found.push((n as f64 / ws.len() as f64, "goal".into(), g.id()));
            }
        }
    }
    // A stable sort keeps the index's order among equal scores.
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out = vec![];
    for (w, kind, id) in found {
        if out.len() >= f.limit {
            break;
        }
        let Some((mut c, open)) = card(app, &kind, id)? else { continue };
        if f.open && !open {
            continue;
        }
        c["score"] = json!((w * 100.0).round() / 100.0);
        out.push(c);
    }
    Ok(out)
}

/// A search hit's card, and whether it's still open (None when it's no longer on the board). A note
/// carries its goal's ref and state: `tb goal show` reads it.
fn card(app: &App, kind: &str, id: i64) -> Result<Option<(Value, bool)>> {
    let goal = |gid: Option<i64>| crate::board::find_goal(app, gid);
    let (c, open) = match kind {
        "task" => {
            let Some(t) = app.db.q1("SELECT * FROM tasks WHERE id = ?", p![id])? else { return Ok(None) };
            let status = t.st("status");
            let state = if t.b("failed") && status != "done" { "failed".to_string() } else { status.clone() };
            let g = goal(t.i("goal_id"))?;
            (json!({"kind": "task", "id": id, "ref": rf("task", id), "title": t.v("title"), "state": state,
                    "project": t.v("project"), "goal": crate::board::goal_ref(g.as_ref())}), status != "done")
        }
        "issue" => {
            let Some(b) = app.db.q1("SELECT * FROM issues WHERE id = ?", p![id])? else { return Ok(None) };
            let g = goal(b.i("goal_id"))?;
            (json!({"kind": "issue", "id": id, "ref": rf("issue", id), "title": b.v("title"), "state": b.v("state"),
                    "issue_kind": b.v("kind"), "project": b.v("project"), "goal": crate::board::goal_ref(g.as_ref())}),
             b.s("state") == Some("open"))
        }
        "goal" => {
            let Some(g) = goal(Some(id))? else { return Ok(None) };
            let state = goal_state(app, &g)?;
            (json!({"kind": "goal", "id": id, "ref": rf("goal", id), "title": g.v("name"), "state": state,
                    "project": g.v("project"), "goal": Value::Null}), state == "open")
        }
        "note" => {
            let Some(n) = app.db.q1("SELECT * FROM goal_notes WHERE id = ?", p![id])? else { return Ok(None) };
            let Some(g) = goal(n.i("goal_id"))? else { return Ok(None) };
            let state = goal_state(app, &g)?;
            (json!({"kind": "note", "id": id, "ref": rf("goal", g.id()), "title": short(&n.st("text"), 120), "state": state,
                    "note_kind": n.v("kind"), "project": g.v("project"), "goal": crate::board::goal_ref(Some(&g))}),
             state == "open")
        }
        "idea" => {
            let Some(i) = app.db.q1("SELECT * FROM ideas WHERE id = ?", p![id])? else { return Ok(None) };
            let g = goal(i.i("goal_id"))?;
            let state = i.st("state");
            (json!({"kind": "idea", "id": id, "ref": rf("idea", id), "title": i.v("title"), "state": state,
                    "project": i.v("project"), "goal": crate::board::goal_ref(g.as_ref())}),
             crate::ideas::OPEN.contains(&state.as_str()))
        }
        _ => return Ok(None),
    };
    Ok(Some((c, open)))
}

/// A goal's state for search: archived, done (every task done) or open.
fn goal_state(app: &App, g: &Row) -> Result<String> {
    if g.b("archived") {
        return Ok("archived".into());
    }
    let total = app.db.count("SELECT COUNT(*) FROM tasks WHERE goal_id = ?", p![g.id()])?;
    let done = app.db.count("SELECT COUNT(*) FROM tasks WHERE goal_id = ? AND status = 'done'", p![g.id()])?;
    Ok(if total > 0 && done == total { "done" } else { "open" }.into())
}

// ---- code areas ----

/// A path's folder ("" at the top).
fn dir(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// The files the work is likely to touch: those it has changed already, paths its text names, and
/// those changed by its goal's other tasks and by the past tasks most like it.
fn work_files(app: &App, w: &Work, task_hits: &[&Hit]) -> Result<HashSet<String>> {
    let mut out: HashSet<String> = HashSet::new();
    let me = w.task.map(|t| t.id());
    if let Some(t) = w.task {
        out.extend(paths_of(app, t.id())?);
        if let Some(g) = t.i("goal_id") {
            for r in app.db.q("SELECT p.path FROM task_paths p JOIN tasks t ON t.id = p.task_id WHERE t.goal_id = ? AND t.id != ?", p![g, t.id()])? {
                out.insert(r.st("path"));
            }
        }
    }
    for h in task_hits.iter().filter(|h| Some(h.rid) != me).take(5) {
        out.extend(paths_of(app, h.rid)?);
    }
    for word in w.text.split_whitespace().map(|s| s.trim_matches(|c: char| !(c.is_alphanumeric() || "/._-".contains(c))).trim_end_matches('.')) {
        if word.contains('/') && word.len() > 2 {
            out.insert(word.trim_matches('/').to_string());
        } else if word.contains('.') && word.chars().next().is_some_and(|c| c.is_alphabetic()) {
            // A file named on its own (lessons.rs): every recorded path with that name.
            let like = format!("%/{word}");
            for r in app.db.q("SELECT DISTINCT path FROM task_paths WHERE path = ? OR path LIKE ? LIMIT 20", p![word, like])? {
                out.insert(r.st("path"));
            }
        }
    }
    Ok(out)
}

/// The files changed by the work a lesson was learned from (its evidence tasks, and its goals' tasks).
fn lesson_files(app: &App, evidence: &[String]) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    for r in evidence {
        let Some(id) = r.get(1..).and_then(|n| n.parse::<i64>().ok()) else { continue };
        let rows = match r.chars().next() {
            Some('T') => app.db.q("SELECT path FROM task_paths WHERE task_id = ?", p![id])?,
            Some('G') => app.db.q("SELECT p.path FROM task_paths p JOIN tasks t ON t.id = p.task_id WHERE t.goal_id = ?", p![id])?,
            _ => continue,
        };
        out.extend(rows.iter().map(|r| r.st("path")));
    }
    Ok(out)
}

/// How far the lesson's files overlap the work's: shared files count fully, shared folders a little.
fn overlap(work: &HashSet<String>, lesson: &HashSet<String>) -> (f64, Vec<String>) {
    if work.is_empty() || lesson.is_empty() {
        return (0.0, vec![]);
    }
    // A path the work names may be a folder: it covers every lesson file under it.
    let covers = |w: &str, l: &str| w == l || l.starts_with(&format!("{w}/"));
    let files: Vec<&String> = lesson.iter().filter(|l| work.iter().any(|w| covers(w, l))).collect();
    let work_dirs: HashSet<&str> = work.iter().flat_map(|w| [w.as_str(), dir(w)]).collect();
    let lesson_dirs: HashSet<&str> = lesson.iter().map(|l| dir(l)).collect();
    let dirs: Vec<&str> = lesson_dirs.iter().filter(|d| work_dirs.contains(*d)).copied().collect();
    let score = files.len() as f64 / lesson.len() as f64 + 0.3 * dirs.len() as f64 / lesson_dirs.len() as f64;
    let mut shown: Vec<String> = files.iter().map(|s| s.to_string()).collect();
    shown.sort();
    if shown.is_empty() {
        shown = dirs.iter().filter(|d| !d.is_empty()).map(|d| format!("{d}/")).collect();
        shown.sort();
    }
    shown.truncate(3);
    (score, shown)
}

// ---- ranking ----

/// The lessons, best fit for the work first (ties: the most confirmed first, as given).
pub fn rank(app: &App, w: &Work, lessons: Vec<Row>) -> Result<Vec<Scored>> {
    let hs = hits(app, w.project, &w.text)?;
    let task_hits: Vec<&Hit> = hs.iter().filter(|h| h.kind == "task").collect();
    let files = work_files(app, w, &task_hits)?;
    let me = w.task.map(|t| t.id());
    let mut text: HashMap<i64, f64> = HashMap::new();
    let mut tasks: HashMap<i64, f64> = HashMap::new();
    let mut goals: HashMap<i64, f64> = HashMap::new();
    for h in &hs {
        match h.kind.as_str() {
            "lesson" => {
                text.insert(h.rid, h.weight);
            }
            "task" if Some(h.rid) != me => {
                tasks.entry(h.rid).or_insert(h.weight);
            }
            "note" => {
                if let Some(g) = h.goal {
                    let e = goals.entry(g).or_insert(0.0);
                    *e = e.max(h.weight);
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for l in lessons {
        let ev: Vec<String> = jloads_arr(l.s("evidence")).iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
        let mut score = 0.0;
        let mut why = vec![];
        if let Some(s) = text.get(&l.id()).filter(|s| **s > 0.0) {
            score += s;
            why.push("its words".to_string());
        }
        // Its evidence reads like the work: half as much as the lesson itself would.
        let mut like: Vec<(f64, &String)> = ev
            .iter()
            .filter_map(|r| {
                let id = r.get(1..)?.parse::<i64>().ok()?;
                let s = match r.chars().next()? {
                    'T' => tasks.get(&id),
                    'G' => goals.get(&id),
                    _ => None,
                }?;
                Some((*s, r))
            })
            .collect();
        like.sort_by(|a, b| b.0.total_cmp(&a.0));
        if let Some((s, _)) = like.first() {
            score += 0.5 * s;
            why.push(format!("like {}", like.iter().take(3).map(|(_, r)| r.as_str()).collect::<Vec<_>>().join(", ")));
        }
        let (o, shared) = overlap(&files, &lesson_files(app, &ev)?);
        if o > 0.0 {
            score += o;
            why.push(format!("touches {}", shared.join(", ")));
        }
        out.push(Scored { lesson: l, score, why });
    }
    // A stable sort keeps the given order (most confirmed first) among equal scores.
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

/// The few lessons worth a prompt's room: the best fits, and when too few fit, the most confirmed
/// of the rest up to `floor`.
pub fn pick(app: &App, w: &Work, lessons: Vec<Row>, count: usize, floor: usize) -> Result<Vec<Row>> {
    const FITS: f64 = 0.15;
    let ranked = rank(app, w, lessons)?;
    let fits = ranked.iter().filter(|s| s.score >= FITS).count().min(count);
    let mut chosen: Vec<&Scored> = ranked.iter().take(fits).collect();
    if chosen.len() < floor {
        let mut rest: Vec<&Scored> = ranked.iter().skip(fits).collect();
        // Back to the most confirmed first.
        rest.sort_by_key(|s| std::cmp::Reverse(s.lesson.i0("confirmations")));
        chosen.extend(rest.into_iter().take(floor - chosen.len()));
    }
    Ok(chosen.into_iter().map(|s| s.lesson.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(xs: &[&str]) -> HashSet<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn queries_keep_the_words_that_matter() {
        assert_eq!(fts_query("Fix the lessons.rs FTS5 index for T12").as_deref(), Some("\"fix\" OR \"lessons\" OR \"fts5\" OR \"index\" OR \"t12\""));
        assert_eq!(fts_query("the and 42 a"), None);
        assert_eq!(fts_query("\"quoted\" OR NEAR(x)").as_deref(), Some("\"quoted\" OR \"near\""));
    }

    #[test]
    fn shared_files_count_more_than_shared_folders() {
        let lesson = set(&["src/db.rs", "src/api.rs"]);
        let (same, shown) = overlap(&set(&["src/db.rs"]), &lesson);
        let (near, _) = overlap(&set(&["src/other.rs"]), &lesson);
        let (folder, _) = overlap(&set(&["src"]), &lesson);
        let (far, _) = overlap(&set(&["docs/x.md"]), &lesson);
        assert_eq!(shown, vec!["src/db.rs"]);
        assert!(same > near && near > 0.0 && far == 0.0, "{same} {near} {far}");
        assert!(folder >= 1.0, "a named folder covers its files: {folder}");
        assert_eq!(overlap(&set(&[]), &lesson).0, 0.0);
    }

    // ---- board search ----

    fn board() -> (tempfile::TempDir, std::sync::Arc<App>) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(crate::config::Config::for_tests(dir.path()));
        (dir, app)
    }

    fn issue(app: &App, project: &str, title: &str, detail: &str) -> i64 {
        app.db.insert("issues", crate::fields!["project" => project, "kind" => "bug", "title" => title, "detail" => detail, "state" => "open"]).unwrap()
    }

    fn indexed(app: &App, id: i64) -> Option<String> {
        app.db.q1("SELECT text FROM search WHERE rowid = ? AND kind = 'issue'", p![id * 4]).unwrap().map(|r| r.st("text"))
    }

    fn search(app: &App, pairs: &[(&str, &str)]) -> Vec<Value> {
        let q: crate::api::Query = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        crate::api::dispatch(app, "GET", "/search", &q, &json!({})).unwrap()["hits"].as_array().unwrap().clone()
    }

    fn refs(hits: &[Value]) -> Vec<String> {
        hits.iter().map(|h| format!("{} {}", h["kind"].as_str().unwrap(), h["ref"].as_str().unwrap())).collect()
    }

    #[test]
    fn backlog_issues_are_indexed_as_they_change() {
        let (_d, app) = board();
        let b = issue(&app, "webapp", "Login button stalls", "Spinner never stops.");
        assert_eq!(indexed(&app, b).as_deref(), Some("Login button stalls\nSpinner never stops.\n"));
        app.db.update("issues", &json!(b), crate::fields!["said" => "it hangs on Safari"]).unwrap();
        assert!(indexed(&app, b).unwrap().ends_with("it hangs on Safari"));
        // A change to a column that isn't searched leaves the row as it was.
        app.db.update("issues", &json!(b), crate::fields!["state" => "drop"]).unwrap();
        assert_eq!(app.db.count("SELECT COUNT(*) FROM search WHERE kind = 'issue'", p![]).unwrap(), 1);
        app.db.x("DELETE FROM issues WHERE id = ?", p![b]).unwrap();
        assert_eq!(indexed(&app, b), None);
    }

    #[test]
    fn issues_made_before_the_index_are_backfilled_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.db");
        let b = {
            let db = crate::db::Db::open(&path).unwrap();
            for t in ["ai", "au", "ad"] {
                db.x(&format!("DROP TRIGGER search_issue_{t}"), p![]).unwrap();
            }
            db.x("DELETE FROM settings WHERE key = 'search_issues_indexed'", p![]).unwrap();
            db.insert("issues", crate::fields!["project" => "webapp", "title" => "Old flaky test", "state" => "open"]).unwrap()
        };
        let db = crate::db::Db::open(&path).unwrap();
        assert_eq!(db.q1("SELECT text FROM search WHERE rowid = ?", p![b * 4]).unwrap().map(|r| r.st("text")).as_deref(), Some("Old flaky test\n\n"));
        db.x("DELETE FROM search WHERE rowid = ?", p![b * 4]).unwrap();
        drop(db);
        let db = crate::db::Db::open(&path).unwrap();
        assert_eq!(db.count("SELECT COUNT(*) FROM search WHERE kind = 'issue'", p![]).unwrap(), 0, "only once");
    }

    #[test]
    fn ideas_are_indexed_beside_the_rest_and_backfilled_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.db");
        let i = {
            let db = crate::db::Db::open(&path).unwrap();
            db.insert("issues", crate::fields!["project" => "webapp", "title" => "Same id as the idea", "state" => "open"]).unwrap();
            for t in ["ai", "au", "ad"] {
                db.x(&format!("DROP TRIGGER search_idea_{t}"), p![]).unwrap();
            }
            db.x("DELETE FROM settings WHERE key = 'search_ideas_indexed'", p![]).unwrap();
            db.insert("ideas", crate::fields!["project" => "webapp", "title" => "Offline mode", "detail" => "Queue writes."]).unwrap()
        };
        let db = crate::db::Db::open(&path).unwrap();
        let text = |db: &crate::db::Db| db.q1("SELECT text FROM search WHERE kind = 'idea' AND rid = ?", p![i]).unwrap().map(|r| r.st("text"));
        assert_eq!(text(&db).as_deref(), Some("Offline mode\nQueue writes."));
        assert_eq!(db.count("SELECT COUNT(*) FROM search WHERE kind = 'issue'", p![]).unwrap(), 1, "the issue with the same id keeps its row");
        db.update("ideas", &json!(i), crate::fields!["title" => "Offline first"]).unwrap();
        assert_eq!(text(&db).as_deref(), Some("Offline first\nQueue writes."));
        db.x("DELETE FROM search WHERE kind = 'idea'", p![]).unwrap();
        drop(db);
        let db = crate::db::Db::open(&path).unwrap();
        assert_eq!(text(&db), None, "only once");
    }

    #[test]
    fn search_ranks_tasks_issues_goals_and_notes_by_their_words() {
        let (_d, app) = board();
        let g = app.db.insert("goals", crate::fields!["name" => "Faster login", "project" => "webapp"]).unwrap();
        let t = app.db.insert("tasks", crate::fields!["title" => "Cache the login token", "detail" => "Login is slow.", "project" => "webapp",
                                                       "status" => "queued", "goal_id" => g]).unwrap();
        let b = issue(&app, "webapp", "Token expires early", "Seen after a long weekend away from the desk.");
        let n = app.db.insert("goal_notes", crate::fields!["goal_id" => g, "kind" => "decision", "text" => "Keep the token in the keychain."]).unwrap();
        issue(&app, "webapp", "Dark mode colours", "");

        let hits = search(&app, &[("q", "login token"), ("project", "webapp")]);
        let mut all = refs(&hits);
        all.sort();
        assert_eq!(all, [format!("goal G{g}"), format!("issue B{b}"), format!("note G{g}"), format!("task T{t}")], "{hits:?}");
        assert_eq!(refs(&hits)[0], format!("task T{t}"), "both words beat one: {hits:?}");
        let of = |kind: &str| hits.iter().find(|h| h["kind"] == kind).unwrap();
        assert_eq!(of("task")["state"], "queued");
        assert_eq!(of("task")["goal"]["ref"], format!("G{g}"));
        assert_eq!(of("issue")["state"], "open");
        assert_eq!(of("goal")["state"], "open");
        assert_eq!(of("note")["id"], n);
        assert_eq!(of("note")["title"], "Keep the token in the keychain.");
        let scores: Vec<f64> = hits.iter().map(|h| h["score"].as_f64().unwrap()).collect();
        assert!(scores.windows(2).all(|w| w[0] >= w[1]), "best first: {scores:?}");
    }

    #[test]
    fn search_keeps_to_the_project_kind_and_open_things_asked_for() {
        let (_d, app) = board();
        let mine = issue(&app, "webapp", "Retry flaky upload", "");
        let other = issue(&app, "api", "Retry flaky webhook", "");
        let done = app.db.insert("tasks", crate::fields!["title" => "Retry flaky download", "project" => "webapp", "status" => "done"]).unwrap();

        assert_eq!(refs(&search(&app, &[("q", "retry flaky"), ("project", "webapp")])).len(), 2);
        let every = refs(&search(&app, &[("q", "retry flaky"), ("project", "all")]));
        assert!(every.contains(&format!("issue B{other}")) && every.len() == 3, "{every:?}");
        assert_eq!(refs(&search(&app, &[("q", "retry flaky"), ("project", "webapp"), ("kind", "backlog")])), [format!("issue B{mine}")]);
        assert_eq!(refs(&search(&app, &[("q", "retry flaky"), ("project", "webapp"), ("kind", "task")])), [format!("task T{done}")]);
        assert_eq!(refs(&search(&app, &[("q", "retry flaky"), ("project", "webapp"), ("open", "1")])), [format!("issue B{mine}")]);

        let q: crate::api::Query = [("q".to_string(), "the and".to_string())].into();
        assert!(crate::api::dispatch(&app, "GET", "/search", &q, &json!({})).is_err(), "no words to search");
        let q: crate::api::Query = [("q".to_string(), "retry".to_string()), ("kind".to_string(), "lesson".to_string())].into();
        assert!(crate::api::dispatch(&app, "GET", "/search", &q, &json!({})).is_err(), "not a kind");
    }
}
