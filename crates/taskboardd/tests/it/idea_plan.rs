//! Planning an idea: a read-only headless Claude drafts a goal and its tasks from the idea, its notes
//! and its dig reports; the board makes them planned (nothing starts) and the idea becomes the goal.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{fields, p};
use taskboardd::util::RowExt;

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let claude = dir.path().join("fake-claude");
    std::fs::write(&claude, "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" >> \"$d/all-args.txt\"\ncat \"$d/answer.json\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = claude.to_string_lossy().to_string();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, dir }
}

fn report() -> Value {
    json!({"blocks": [
        {"type": "gist", "text": "The store is there; the sync isn't."},
        {"type": "scorecard", "fit": "partly", "size": "m", "risk": "medium"},
        {"type": "code", "items": [{"what": "The write queue", "file": "src/queue.rs"}]},
        {"type": "points", "title": "Watch out", "items": [{"short": "Writes on disk", "detail": "Keeps writes on disk", "tag": "privacy"}]},
        {"type": "question", "id": "conflicts", "text": "Last write wins?", "choices": ["Yes", "Ask"]}]})
}

/// A report kept before reports were blocks.
fn old_report() -> Value {
    json!({"verdict": "fits", "why": "It fits.", "builds_on": [], "ways_to_extend": ["Sync on a schedule"], "risks": [],
        "pitfalls": [], "open_questions": [], "related": []})
}

fn plan() -> Value {
    json!({"name": "Offline mode", "tldr": "Writes wait while offline and go out once back.",
        "outcome": "Done when a write made offline reaches the server after reconnecting.",
        "tasks": [
            {"title": "Queue writes while offline", "detail": "Review: the migration.", "after": [], "stack_on": 0, "stack_merge": "tail"},
            {"title": "Send queued writes on reconnect", "detail": "", "after": [1], "stack_on": 1, "stack_merge": "head"}]})
}

impl Board {
    fn try_post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        self.try_post(path, body).unwrap_or_else(|e| panic!("POST {path}: {e:?}"))
    }
    fn idea(&self) -> String {
        let v = self.post("/ideas", json!({"title": "Offline mode", "detail": "Queue writes while offline.", "project": "webapp"}));
        v["ref"].as_str().unwrap().to_string()
    }
    fn answer(&self, v: Value) {
        std::fs::write(self.dir.path().join("answer.json"), json!({"structured_output": v}).to_string()).unwrap();
    }
    /// Claude's arguments since the last call, cleared.
    fn calls(&self) -> String {
        let f = self.dir.path().join("all-args.txt");
        let s = std::fs::read_to_string(&f).unwrap_or_default();
        let _ = std::fs::remove_file(f);
        s
    }
}

#[test]
fn a_plan_makes_a_planned_goal_and_the_idea_becomes_it() {
    let b = board();
    let i = b.idea();
    b.post(&format!("/ideas/{i}/note"), json!({"text": "Only for the inbox at first."}));
    let id: i64 = i[1..].parse().unwrap();
    b.app.db.insert("idea_rounds", fields!["idea_id" => id, "focus" => "", "web" => 0, "started_at" => "2026-10-19T12:00:00Z",
        "finished_at" => "2026-10-19T12:10:00Z", "report" => old_report().to_string()]).unwrap();
    b.answer(report());
    b.post(&format!("/ideas/{i}/dig"), json!({}));
    b.calls();
    b.answer(plan());
    let v = b.post(&format!("/ideas/{i}/plan"), json!({}));

    let args = b.calls();
    assert!(args.contains("--tools\nRead,Grep,Glob\n"), "read-only tools: {args}");
    assert!(!args.contains("WebSearch") && !args.contains("Bash") && !args.contains("Edit"), "{args}");
    assert!(args.contains("Queue writes while offline.") && args.contains("Only for the inbox at first."), "detail and notes: {args}");
    assert!(args.contains("Last write wins?") && args.contains("src/queue.rs") && args.contains("Keeps writes on disk"), "the dig report: {args}");
    assert!(args.contains("Sync on a schedule"), "a report from before blocks: {args}");
    assert!(args.contains("How to slice the work:") && args.contains("--stack-merge"), "the planning rules: {args}");

    assert_eq!(v["state"], "planned");
    assert_eq!(v["planning"], false);
    let g = v["goal"].as_str().expect("the idea has its goal");
    let gid: i64 = g[1..].parse().unwrap();
    let goal = b.app.db.q1("SELECT * FROM goals WHERE id = ?", p![gid]).unwrap().unwrap();
    assert_eq!((goal.st("name"), goal.st("project")), ("Offline mode".to_string(), "webapp".to_string()));
    assert!(goal.st("outcome").starts_with("Done when"));
    assert_eq!(goal.st("tldr"), "Writes wait while offline and go out once back.");
    let note = b.app.db.q1("SELECT * FROM goal_notes WHERE goal_id = ?", p![gid]).unwrap().unwrap();
    assert_eq!((note.st("kind"), note.i("pinned")), ("decision".to_string(), Some(1)));
    assert!(note.st("text").contains(&format!("Planned from idea {i}")) && note.st("text").contains("its 2 dig reports"), "{}", note.st("text"));

    let tasks = b.app.db.q("SELECT * FROM tasks WHERE goal_id = ? ORDER BY id", p![gid]).unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|t| t.st("status") == "planned"), "planned, never queued or started");
    assert_eq!(serde_json::from_str::<Value>(&tasks[1].st("waits_for")).unwrap(), json!([tasks[0].id()]));
    assert_eq!((tasks[1].i("pr_after"), tasks[1].st("stack_merge")), (Some(tasks[0].id()), "head".to_string()));
    assert_eq!(tasks[0].i("pr_after"), None);

    let (code, msg) = b.try_post(&format!("/ideas/{i}/plan"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains(&format!("became {g} already")), "{msg}");
    let (code, _) = b.try_post(&format!("/ideas/{i}/dig"), json!({})).unwrap_err();
    assert_eq!(code, 409, "a planned idea isn't dug into");
}

#[test]
fn a_failed_plan_leaves_the_idea_open_with_a_note() {
    let b = board();
    let i = b.idea();
    b.answer(json!({"name": "Offline mode", "outcome": "Done when …", "tasks": []}));
    let v = b.post(&format!("/ideas/{i}/plan"), json!({}));
    assert_eq!(v["state"], "new");
    assert_eq!(v["goal"], Value::Null);
    let note = v["notes"][0]["text"].as_str().unwrap();
    assert!(note.starts_with("Claude couldn't plan this") && note.contains("no tasks"), "{note}");
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM goals", p![]).unwrap(), 0, "no goal is left half made");
    assert!(b.app.shared.lock().idea_planning.is_empty());
}

#[test]
fn no_plan_while_digging_or_put_aside() {
    let b = board();
    let i = b.idea();
    let id: i64 = i[1..].parse().unwrap();
    b.app.shared.lock().digging.insert(id);
    let (code, msg) = b.try_post(&format!("/ideas/{i}/plan"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("being dug into"), "{msg}");
    b.app.shared.lock().digging.remove(&id);

    b.app.shared.lock().idea_planning.insert(id);
    let (code, msg) = b.try_post(&format!("/ideas/{i}/plan"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("being planned already"), "{msg}");
    let (code, msg) = b.try_post(&format!("/ideas/{i}/dig"), json!({})).unwrap_err();
    assert_eq!(code, 409, "no dig while it's planned: {msg}");
    let (code, _) = b.try_post(&format!("/ideas/{i}/shelve"), json!({})).unwrap_err();
    assert_eq!(code, 409, "not put aside while it's planned");
    assert_eq!(api::dispatch(&b.app, "GET", &format!("/ideas/{i}"), &Query::new(), &json!({})).unwrap()["planning"], true);
    b.app.shared.lock().idea_planning.remove(&id);

    b.post(&format!("/ideas/{i}/shelve"), json!({}));
    let (code, msg) = b.try_post(&format!("/ideas/{i}/plan"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("reopen it first"), "{msg}");
    assert_eq!(b.calls(), "", "Claude wasn't asked");
}
