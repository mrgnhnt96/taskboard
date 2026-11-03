//! Reviewing a new plan against its goal's outcome: a read-only headless Claude runs once after
//! `tb goal new` or `tb propose` adds tasks, its findings are kept on the goal, and nothing runs
//! when Claude or the review is off.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::reports;

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn board_with(setup: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let claude = dir.path().join("fake-claude");
    std::fs::write(&claude, "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" >> \"$d/all-args.txt\"\ncat \"$d/answer.json\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = claude.to_string_lossy().to_string();
    cfg.backlog.ai = true;
    setup(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    let b = Board { app, dir };
    b.answer(json!({"findings": []}));
    b
}

fn board() -> Board {
    board_with(|_| {})
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut body = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        reports::handle(&self.app, body, false).unwrap()
    }
    /// What `tb goal new … --task` reports.
    fn goal_new(&self) -> String {
        let v = self.report(
            "tb.goal",
            json!({"name": "Dark mode", "project": "webapp", "outcome": "Every screen follows the system theme and the toggle remembers the choice",
                   "tasks": ["Colors::pick the dark palette", "Toggle::add the theme toggle"]}),
        );
        v["goal"].as_str().unwrap().to_string()
    }
    fn propose(&self, goal: &str) -> Value {
        self.report("tb.propose", json!({"goal": goal, "tasks": ["Remember::keep the toggle's choice"]}))
    }
    fn try_post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
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
    fn rounds(&self, goal: &str) -> i64 {
        let id: i64 = goal.trim_start_matches('G').parse().unwrap();
        self.app.db.count("SELECT COUNT(*) FROM plan_reviews WHERE goal_id = ?", taskboardd::p![id]).unwrap()
    }
    fn task_ref(&self, goal: &str, title: &str) -> String {
        let g = self.get(&format!("/goals/{goal}"));
        let t = g["tasks"].as_array().unwrap().iter().find(|t| t["title"] == title).unwrap_or_else(|| panic!("{title} in {g}"));
        t["ref"].as_str().unwrap().to_string()
    }
}

#[test]
fn a_new_goal_is_reviewed_once_and_its_findings_kept_on_the_goal() {
    let b = board();
    b.answer(json!({"findings": [
        {"kind": "outcome_unproven", "short": "Nothing proves every screen follows the theme", "detail": "Add a task that checks each screen.", "tasks": [], "refs": ["G999"]},
        {"kind": "made_up", "short": "Not a kind", "detail": "", "tasks": [], "refs": []},
    ]}));
    let g = b.goal_new();

    let args = b.calls();
    assert!(args.contains("--tools\nRead,Grep,Glob\n"), "read-only tools: {args}");
    assert!(!args.contains("Bash") && !args.contains("Edit"), "{args}");
    assert!(args.contains("--max-turns\n30\n") && args.contains("--max-budget-usd\n2.00\n"), "room to read the checkout: {args}");
    assert!(args.contains("Check the plan against the checkout before you answer"), "reads before it reports: {args}");
    let schema = args.split("--json-schema\n").nth(1).and_then(|s| s.lines().next()).unwrap_or_default();
    let schema: Value = serde_json::from_str(schema).unwrap_or_else(|e| panic!("{e}: {args}"));
    assert_eq!(schema["required"], json!(["checked", "findings"]), "the checks it made are asked for: {schema}");
    assert!(args.contains("Done when: Every screen follows the system theme"), "the outcome: {args}");
    assert!(args.contains("Colors") && args.contains("pick the dark palette") && args.contains("Toggle"), "the tasks: {args}");
    assert_eq!(b.rounds(&g), 1);

    let review = &b.get(&format!("/goals/{g}/review"))["review"];
    assert_eq!(review["state"], "done", "{review}");
    assert_eq!(review["reason"], "new plan");
    assert_eq!(review["model"], json!(taskboardd::plan_review::model(&b.app)));
    assert_eq!(review["sha"], json!(git(&b.dir.path().join("webapp"), &["rev-parse", "HEAD"])));
    let findings = review["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "unknown kinds are dropped: {review}");
    assert_eq!(findings[0]["kind"], "outcome_unproven");
    assert_eq!(findings[0]["refs"], json!([]), "refs the prompt didn't show are dropped");

    let shown = &b.get(&format!("/goals/{g}"))["plan_review"];
    assert_eq!(shown["findings"], review["findings"], "goal detail carries the same review");
}

#[test]
fn a_review_that_runs_out_of_turns_says_so() {
    let b = board();
    std::fs::write(b.dir.path().join("answer.json"), json!({"subtype": "error_max_turns", "num_turns": 31}).to_string()).unwrap();
    let g = b.goal_new();

    let review = &b.get(&format!("/goals/{g}/review"))["review"];
    assert_eq!(review["state"], "failed", "{review}");
    assert!(review["error"].as_str().unwrap_or_default().contains("ran out of turns (31)"), "{review}");
}

#[test]
fn proposing_tasks_reviews_the_plan_again() {
    let b = board();
    let g = b.goal_new();
    b.calls();
    let colors = b.task_ref(&g, "Colors");
    b.answer(json!({"findings": [{"kind": "past_decision", "short": "Toggle waits on the palette", "detail": "", "tasks": [colors, "T99999"], "refs": []}]}));
    b.propose(&g);

    let args = b.calls();
    assert!(args.contains("keep the toggle's choice"), "the new task is in the plan: {args}");
    assert_eq!(b.rounds(&g), 2);
    let review = &b.get(&format!("/goals/{g}/review"))["review"];
    assert_eq!(review["reason"], "the plan changed");
    assert_eq!(review["findings"][0]["tasks"], json!([colors]), "only the goal's own tasks are kept: {review}");
}

#[test]
fn asking_for_a_review_runs_it_again() {
    let b = board();
    let g = b.goal_new();
    b.calls();
    let v = b.try_post(&format!("/goals/{g}/review"), json!({})).unwrap();
    assert_eq!(v["review"]["reason"], "asked");
    assert_eq!(v["review"]["state"], "done");
    assert!(!b.calls().is_empty(), "claude ran again");
    assert_eq!(b.rounds(&g), 2);
}

#[test]
fn with_claude_off_nothing_runs_and_the_review_says_why() {
    let b = board_with(|c| c.backlog.ai = false);
    let g = b.goal_new();
    assert!(b.calls().is_empty(), "claude never ran");
    let review = &b.get(&format!("/goals/{g}"))["plan_review"];
    assert_eq!(review["state"], "skipped", "{review}");
    assert!(review["skipped"].as_str().unwrap().contains("Claude isn't available"), "{review}");

    let (status, msg) = b.try_post(&format!("/goals/{g}/review"), json!({})).unwrap_err();
    assert_eq!(status, 409);
    assert!(msg.contains("can't be reviewed now"), "{msg}");
    assert!(b.calls().is_empty());
}

#[test]
fn with_the_review_off_nothing_runs() {
    let b = board_with(|c| c.planning.review = false);
    let g = b.goal_new();
    b.propose(&g);
    assert!(b.calls().is_empty(), "claude never ran");
    let review = &b.get(&format!("/goals/{g}/review"))["review"];
    assert_eq!(review["state"], "skipped");
    assert!(review["skipped"].as_str().unwrap().contains("plan review is off"), "{review}");
}

#[test]
fn with_the_usage_used_up_nothing_runs() {
    let b = board();
    let resets = taskboardd::util::iso(taskboardd::util::now_ts() + 3600.0);
    b.app.shared.lock().midna_usage = Some(json!({"five_hour": {"used_percentage": 100, "resets_at": resets}}));
    let g = b.goal_new();
    assert!(b.calls().is_empty(), "claude never ran");
    let review = &b.get(&format!("/goals/{g}/review"))["review"];
    assert_eq!(review["state"], "skipped");
    assert_eq!(review["skipped"], "the usage is used up");
}

#[test]
fn a_goal_with_no_tasks_has_no_plan_to_review() {
    let b = board();
    let g = b.report("tb.goal", json!({"name": "Empty", "project": "webapp"}))["goal"].as_str().unwrap().to_string();
    assert!(b.calls().is_empty());
    assert!(b.get(&format!("/goals/{g}"))["plan_review"].is_null());
    let (status, msg) = b.try_post(&format!("/goals/{g}/review"), json!({})).unwrap_err();
    assert_eq!(status, 409);
    assert!(msg.contains("no tasks yet"), "{msg}");
}

#[test]
fn work_a_task_records_as_done_or_proves_itself_is_not_a_gap() {
    let b = board();
    b.report(
        "tb.goal",
        json!({"name": "Store sync", "project": "webapp", "outcome": "Every store page loads under a second, with timings attached",
               "tasks": ["Sync::call the store API with the key Morgan already made; attach the page timings as results"]}),
    );
    let args = b.calls();
    assert!(args.contains("with the key Morgan already made"), "the task's record of the done step reaches Claude: {args}");
    assert!(args.contains("record as already done") && args.contains("isn't one."), "a done human step isn't an owner prerequisite: {args}");
    assert!(
        args.contains("attaching its own results") && args.contains("don't ask for a separate task to prove it"),
        "a clause a task proves with its own results isn't unproven: {args}"
    );
}

#[test]
fn every_item_of_a_source_the_outcome_points_at_reaches_claude() {
    let b = board();
    let g = b.report("tb.goal", json!({"name": "Budget review", "project": "webapp", "outcome": "The app matches the Nov 1 review and B900 is closed"}))
        ["goal"].as_str().unwrap().to_string();
    let review = b.dir.path().join("review.csv");
    std::fs::write(&review, "item,decision\n1,Pay Southwest first\n2,Tithing comes first from every paycheck\n3,Link X Money once\n").unwrap();
    b.report("tb.attach", json!({"goal": g, "url": review.to_string_lossy(), "title": "Nov 1 review", "kind": "doc"}));
    b.app
        .db
        .x(
            "INSERT INTO issues(id, project, kind, title, detail, state) VALUES (900, 'webapp', 'bug', 'Frozen month loses income', 'Income sorted late gets no tithing', 'open')",
            taskboardd::p![],
        )
        .unwrap();
    b.calls();
    b.report("tb.propose", json!({"goal": g, "tasks": ["Pay order::pay Southwest first"]}));

    let args = b.calls();
    assert!(args.contains("--tools\nRead,Grep,Glob\n"), "no link, so no WebFetch: {args}");
    assert!(args.contains("List every item it asks for") && args.contains("not only the ones the tasks name"), "the rule: {args}");
    assert!(args.contains("Nov 1 review") && args.contains("| 2,Tithing comes first from every paycheck") && args.contains("| 3,Link X Money once"),
        "the attached review's items are inlined: {args}");
    assert!(args.contains("B900 [open] bug: Frozen month loses income") && args.contains("Income sorted late gets no tithing"),
        "the backlog issue the outcome names: {args}");

    b.answer(json!({"findings": [{"kind": "outcome_unproven", "short": "Review item 3 (X Money) has no task", "detail": "", "tasks": [], "refs": ["B900", "B901"]}]}));
    b.try_post(&format!("/goals/{g}/review"), json!({})).unwrap();
    let review = &b.get(&format!("/goals/{g}/review"))["review"];
    assert_eq!(review["findings"][0]["refs"], json!(["B900"]), "a named issue may be a finding's ref: {review}");
}

#[test]
fn a_linked_source_lets_claude_open_it() {
    let b = board();
    let g = b.report("tb.goal", json!({"name": "Review fixes", "project": "webapp", "outcome": "Every point in https://github.com/acme/webapp/pull/7#pullrequestreview-1 is fixed"}))
        ["goal"].as_str().unwrap().to_string();
    b.report("tb.attach", json!({"goal": g, "url": "https://claude.ai/artifact/abc", "title": "Screens", "kind": "design"}));
    b.calls();
    b.report("tb.propose", json!({"goal": g, "tasks": ["Fix::fix the review's points"]}));
    let args = b.calls();
    assert!(args.contains("--tools\nRead,Grep,Glob,WebFetch\n"), "the outcome's link can be opened: {args}");
    assert!(args.contains("Open a source's link with WebFetch"), "{args}");
    assert!(args.contains("design: Screens https://claude.ai/artifact/abc") && args.contains("the board can't open"), "an artifact is judged by its title: {args}");
}
