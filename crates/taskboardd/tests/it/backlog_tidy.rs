//! The backlog tidy: on a timer, Claude closes open issues that repeat another or aren't real,
//! reading the ones read longest ago first, and skips a project with nothing added and no merge
//! since its last tidy.

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

#[test]
fn closes_issues_left_in_finished_goals() {
    let b = board();
    let goal = |name: &str| b.post("/goals", json!({"name": name, "project": "webapp"}))["id"].as_i64().unwrap();
    let in_goal = |g: i64, title: &str| {
        let id = b.issue(title);
        b.app.db.x("UPDATE issues SET goal_id = ? WHERE id = ?", p![g, id]).unwrap();
        id
    };
    let (done, archived, live) = (goal("Runner"), goal("Retries"), goal("Logs"));
    b.app.db.x("INSERT INTO tasks(title, project, goal_id, status) VALUES('Fix the runner', 'webapp', ?, 'done')", p![done]).unwrap();
    b.app.db.x("UPDATE goals SET archived = 1 WHERE id = ?", p![archived]).unwrap();
    b.app.db.x("INSERT INTO tasks(title, project, goal_id, status) VALUES('Tidy the logs', 'webapp', ?, 'working')", p![live]).unwrap();
    let fixed = in_goal(done, "Runner drops the last line of output");
    let stale = in_goal(archived, "Retry never backs off");
    let target = in_goal(live, "Log lines lose their timestamp");
    let dup = in_goal(done, "Timestamps go missing from log lines");
    let kept = in_goal(live, "Logs rotate too often");
    b.answer(json!({"close": [
        {"id": fixed, "why": "fixed", "reason": "run.rs flushes the last line"},
        {"id": stale, "why": "false", "reason": "retry.rs doubles the wait"},
        {"id": dup, "why": "duplicate", "of": target, "reason": "Same missing timestamp"},
        {"id": kept, "why": "fixed", "reason": "in a goal still running"}]}));
    tidy::tick(&b.app).unwrap();

    let prompt = b.calls();
    let (may, never) = prompt.split_once("never close them").expect("the live goal's issues are listed as planned");
    for id in [fixed, stale, dup] {
        assert!(may.contains(&format!("B{id}")), "B{id} may be closed: {prompt}");
    }
    assert!(never.contains(&format!("B{target}")) && never.contains(&format!("B{kept}")), "{prompt}");
    let states: Vec<String> = [fixed, stale, dup, target, kept].iter().map(|id| b.state(*id)).collect();
    assert_eq!(states, ["drop", "drop", "drop", "open", "open"], "issues in a finished or archived goal close; a running goal's stay");
    assert!(b.log(target).iter().any(|l| l == &format!("The backlog tidy closed B{dup} as a duplicate of this")));
}

impl Board {
    /// Runs git in the project's checkout.
    fn git(&self, args: &[&str]) {
        let repo = self.dir.path().join("webapp");
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }
    /// A commit on the checkout's main, as a merged PR leaves.
    fn merge(&self, what: &str) {
        self.git(&["commit", "-q", "--allow-empty", "-m", what]);
    }
}

#[test]
fn tidies_again_once_main_moves() {
    let b = board();
    b.git(&["init", "-q", "-b", "main"]);
    b.merge("first");
    let id = b.issue("Runner drops the last line of output");
    tidy::tick(&b.app).unwrap();
    assert!(!b.calls().is_empty(), "the first tidy reads the backlog");

    b.time_passes();
    tidy::tick(&b.app).unwrap();
    assert!(b.calls().is_empty(), "nothing added and main where it was: skipped");

    b.merge("Merge pull request #2: flush the last line");
    tidy::tick(&b.app).unwrap();
    assert!(b.calls().is_empty(), "main moved, but not a day since the last tidy");

    b.time_passes();
    tidy::tick(&b.app).unwrap();
    let prompt = b.calls();
    assert!(prompt.contains(&format!("B{id} [bug]")), "main moved: the issue is read again against the new code: {prompt}");
    assert!(prompt.contains("read it again"), "{prompt}");

    b.time_passes();
    tidy::tick(&b.app).unwrap();
    assert!(b.calls().is_empty(), "main hasn't moved since that tidy");
}

#[test]
fn reads_the_issues_read_longest_ago_first() {
    let b = board();
    let ids: Vec<i64> = (1..=152)
        .map(|n| {
            b.app.db.x("INSERT INTO issues(project, kind, title, state) VALUES('webapp', 'bug', ?, 'open')", p![format!("Problem {n}")]).unwrap();
            b.app.db.q("SELECT MAX(id) AS id FROM issues", p![]).unwrap()[0].i0("id")
        })
        .collect();
    let read = |prompt: &str| -> Vec<bool> { ids.iter().map(|id| prompt.contains(&format!("B{id} [bug]"))).collect() };
    let tidied = |id: i64| b.app.db.q("SELECT tidied_at FROM issues WHERE id = ?", p![id]).unwrap()[0].s("tidied_at").map(str::to_string);

    b.post("/backlog/tidy", json!({"now": true}));
    let first = read(&b.calls());
    assert_eq!(first.iter().filter(|r| **r).count(), 150, "at most 150 issues a tidy");
    assert!(!first[0] && !first[1] && first[2..].iter().all(|r| *r), "never read, newest first");
    assert!(tidied(ids[0]).is_none() && tidied(ids[2]).is_some(), "each issue read is marked");

    b.app.db.x("UPDATE issues SET tidied_at = '2020-01-01T00:00:00Z' WHERE id IN (?, ?, ?)", p![ids[2], ids[3], ids[4]]).unwrap();
    b.post("/backlog/tidy", json!({"now": true}));
    let second = read(&b.calls());
    assert!(second[..5].iter().all(|r| *r), "the two never read, then the three read longest ago");
    assert!(!second[5] && !second[6], "the rest by newest, up to 150: {second:?}");
    assert!(tidied(ids[0]).is_some(), "every issue read once over two tidies");
}
