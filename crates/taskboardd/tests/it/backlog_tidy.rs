//! The backlog tidy: on a timer, Claude closes open issues that repeat another or aren't real, and
//! skips a project with nothing added since its last tidy.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, p, tidy};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.backlog.ai = true;
    let claude = dir.path().join("fake-claude");
    std::fs::write(&claude, "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" >> \"$d/all-args.txt\"\ncat \"$d/answer.json\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = claude.to_string_lossy().to_string();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let b = Board { app, dir };
    b.answer(json!({"close": []}));
    b
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn issue(&self, title: &str) -> i64 {
        self.post("/backlog", json!({"title": title, "project": "webapp", "kind": "bug"}))["id"].as_i64().unwrap()
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
    fn state(&self, id: i64) -> String {
        board::get_issue(&self.app, id).unwrap().st("state")
    }
    fn log(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM issue_events WHERE issue_id = ? ORDER BY id", p![id]).unwrap().iter().map(|r| r.st("text")).collect()
    }
    /// As if the last tidy was two days ago.
    fn time_passes(&self) {
        let mut s: Value = serde_json::from_str(&self.app.db.get_setting("backlog_tidy_state").unwrap().unwrap()).unwrap();
        s["checked_at"] = json!("2020-01-01T00:00:00Z");
        self.app.db.set_setting("backlog_tidy_state", Some(&s.to_string())).unwrap();
    }
}

#[test]
fn closes_repeats_and_false_findings_with_the_reason_in_their_log() {
    let b = board();
    let keep = b.issue("Runner drops the last line of output");
    let dup = b.issue("Last output line goes missing in the runner");
    let wrong = b.issue("Retry never backs off");
    b.answer(json!({"close": [
        {"id": dup, "why": "duplicate", "of": keep, "reason": "Same missing line"},
        {"id": wrong, "why": "false", "reason": "retry.rs doubles the wait each time"}]}));
    tidy::tick(&b.app).unwrap();

    let args = b.calls();
    assert!(args.contains("Read,Grep,Glob"), "Claude may read the project's code: {args}");
    assert_eq!((b.state(keep).as_str(), b.state(dup).as_str(), b.state(wrong).as_str()), ("open", "drop", "drop"));
    assert!(b.log(dup).iter().any(|l| l == &format!("The backlog tidy closed it as a duplicate of B{keep}: Same missing line")), "{:?}", b.log(dup));
    assert!(b.log(wrong).iter().any(|l| l.contains("not a real problem: retry.rs doubles")));
    assert!(b.log(keep).iter().any(|l| l == &format!("The backlog tidy closed B{dup} as a duplicate of this")));
    let line = b.get("/backlog/tidy")["line"].as_str().unwrap().to_string();
    assert!(line.starts_with("Backlog tidy: on, every day."), "{line}");
    assert!(line.contains(&format!("closed B{dup} (repeats B{keep}); B{wrong} (not a real problem)")), "{line}");
}

#[test]
fn waits_its_interval_and_skips_when_nothing_was_added() {
    let b = board();
    b.issue("Runner drops the last line of output");
    tidy::tick(&b.app).unwrap();
    assert!(!b.calls().is_empty(), "the first tidy reads the backlog");

    b.issue("Retry never backs off");
    tidy::tick(&b.app).unwrap();
    assert!(b.calls().is_empty(), "not a day since the last tidy");

    b.time_passes();
    tidy::tick(&b.app).unwrap();
    assert!(!b.calls().is_empty(), "an issue was added since");

    b.time_passes();
    tidy::tick(&b.app).unwrap();
    assert!(b.calls().is_empty(), "nothing added since the last tidy: skipped");

    b.post("/backlog/tidy", json!({"now": true}));
    assert!(!b.calls().is_empty(), "tb backlog tidy --now reads it anyway");
}

#[test]
fn turns_off_and_changes_how_often() {
    let b = board();
    let v = b.post("/backlog/tidy", json!({"on": false}));
    assert_eq!(v["line"], "Backlog tidy: off.");
    b.issue("Runner drops the last line of output");
    tidy::tick(&b.app).unwrap();
    assert!(b.calls().is_empty(), "off: no tidy");

    let v = b.post("/backlog/tidy", json!({"on": true, "every_hours": 1}));
    assert!(v["line"].as_str().unwrap().starts_with("Backlog tidy: on, every hour."));
    assert!(api::dispatch(&b.app, "POST", "/backlog/tidy", &Query::new(), &json!({"every_hours": 0.5})).is_err());
    let v = b.post("/backlog/tidy", json!({"reset": true}));
    assert_eq!(v["every_hours"], 24.0);
}

#[test]
fn leaves_planned_issues_alone() {
    let b = board();
    let planned = b.issue("Runner drops the last line of output");
    let g = b.post("/goals", json!({"name": "Runner", "project": "webapp"}))["id"].as_i64().unwrap();
    b.app.db.x("UPDATE issues SET goal_id = ? WHERE id = ?", p![g, planned]).unwrap();
    let dup = b.issue("Last output line goes missing in the runner");
    b.answer(json!({"close": [
        {"id": planned, "why": "duplicate", "of": dup, "reason": "x"},
        {"id": dup, "why": "duplicate", "of": planned, "reason": "Planned in the goal already"}]}));
    tidy::tick(&b.app).unwrap();
    assert_eq!((b.state(planned).as_str(), b.state(dup).as_str()), ("open", "drop"));
}
