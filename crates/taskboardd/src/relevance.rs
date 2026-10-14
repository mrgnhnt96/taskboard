//! Which lessons fit a piece of work. Two things are kept for it, in the board's own database:
//!
//! - `task_paths`: the files each task's PR changed, read with git from the PR's base and head as
//!   the PR moves (`tick`), so a task's code areas outlast its branch.
//! - `search`: an FTS5 index of lessons, goal notes and task titles and details, kept in step by
//!   triggers.
//!
//! A lesson scores on its own text matching the work, on the tasks and goals it was learned from
//! (its evidence) matching it, and on how far the files its evidence changed overlap the files the
//! work is likely to touch: paths the work names, and the files changed by its goal's other tasks
//! and by the past tasks most like it.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

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

/// An FTS5 query matching any of the text's words, or None when it has none worth matching.
pub fn fts_query(text: &str) -> Option<String> {
    let mut seen = HashSet::new();
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|w| w.to_lowercase())
        .filter(|w| w.chars().count() >= 3 && !w.chars().all(|c| c.is_ascii_digit()) && !STOP.contains(&w.as_str()))
        .filter(|w| seen.insert(w.clone()))
        .take(40)
        .map(|w| format!("\"{w}\""))
        .collect();
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
        "SELECT kind, rid, goal, bm25(search) AS rank FROM search WHERE search MATCH ? AND (project IS ? OR project IS NULL)
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
}
