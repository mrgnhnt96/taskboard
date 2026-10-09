//! The Python board's later changes of 2026-10-08 (`docs/parity/oct8-changes.md`, 15–19): locks and
//! tasks that run alone, `#k` waits in planned tasks, a worktree per task, keeping a task off another
//! task's worktree and PR, reopening a done task on a follow-up, and leaving tabs the owner opened.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{PrLink, RowExt};
use taskboardd::{board, midna, p, prflow, reports, runner, worktrees};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        self.try_report(event, session, extra).unwrap_or_else(|e| panic!("{event}: {e:?}"))
    }
    fn try_report(&self, event: &str, session: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn repo(&self) -> String {
        self.dir.path().join("webapp").to_string_lossy().to_string()
    }
    fn add_session(&self, sid: &str) {
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": self.repo(), "status": {"state": "working"}})],
            &[],
        )
        .unwrap();
    }
    fn side_by_side(&self) -> i64 {
        self.post("/goals", json!({"name": "Parallel", "project": "webapp", "run_in_order": false, "max_terminals": 4}))["id"].as_i64().unwrap()
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp", "ships_pr": false});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
    fn card(&self, id: i64) -> Value {
        self.get(&format!("tasks/T{id}"))
    }
    fn start_job(&self, id: i64) -> taskboardd::util::Row {
        let j = self.app.db.q1("SELECT * FROM jobs WHERE kind = 'agent' AND purpose = 'start' AND task_id = ? ORDER BY id DESC", p![id]).unwrap();
        board::job_args(&j.unwrap_or_else(|| panic!("T{id} has no start job")))
    }
    fn started(&self) -> Vec<i64> {
        self.app.db.q("SELECT DISTINCT task_id FROM jobs WHERE kind = 'agent' AND purpose = 'start' ORDER BY task_id", p![]).unwrap().iter().map(|j| j.i0("task_id")).collect()
    }
    /// The terminal the board opened for the task picks it up, then finishes it.
    fn run_to_done(&self, id: i64, sid: &str) {
        self.add_session(sid);
        self.open_for(sid, id);
        self.report("hook.prompt", sid, json!({"prompt": self.start_job(id).st("prompt")}));
        self.report("tb.done", sid, json!({"summary": "Done."}));
    }
    /// Marks an agent job as having opened this terminal.
    fn open_for(&self, sid: &str, id: i64) {
        board::create_job(&self.app, "agent", json!({}), Some(id), "", Some(json!({"session": sid}))).unwrap();
    }
}

#[test]
fn nothing_set_runs_a_wave_side_by_side_as_before() {
    let b = new_board();
    let g = b.side_by_side();
    let ts: Vec<i64> = (1..=3).map(|i| b.task(&format!("Fix {i}"), json!({"goal_id": g}))).collect();
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), ts);
    assert_eq!(b.card(ts[0])["locks"], json!([]));
    assert!(b.card(ts[0])["alone"].is_null());
}

#[test]
fn tasks_sharing_a_lock_take_turns() {
    let b = new_board();
    let g = b.side_by_side();
    let (a, t2, c) = (b.task("A", json!({"goal_id": g})), b.task("B", json!({"goal_id": g})), b.task("C", json!({"goal_id": g})));
    let out = b.post(&format!("tasks/T{a}"), json!({"locks": ["Local-Core"]}));
    assert_eq!(out["locks"], json!(["local-core"]));
    assert!(out["warnings"][0].as_str().unwrap().contains("local-core"), "a lock no other task names gets a spelling hint");
    let out = b.post(&format!("tasks/T{t2}"), json!({"locks": "local-core, emulator-5554"}));
    assert!(out["warnings"].as_array().map(|w| w.iter().all(|w| !w.as_str().unwrap().contains("local-core"))).unwrap_or(true), "{out}");

    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![a, c]);
    assert_eq!(b.card(t2)["waiting"], format!("Waits for local-core (T{a} has it)"));
    let locks = b.get("locks");
    assert_eq!(locks["locks"][1], json!({"name": "local-core", "held_by": format!("T{a}"), "tasks": [format!("T{t2}")]}));

    b.run_to_done(a, "s1");
    runner::start_queued(&b.app).unwrap();
    assert!(b.started().contains(&t2));
    assert!(b.start_job(t2).st("prompt").contains("This task holds local-core, emulator-5554"));
}

#[test]
fn a_lock_is_board_wide_and_names_are_checked() {
    let b = new_board();
    let (g1, g2) = (b.side_by_side(), b.side_by_side());
    let a = b.task("A", json!({"goal_id": g1, "locks": ["emulator-5554"]}));
    let other = b.task("Elsewhere", json!({"goal_id": g2, "locks": ["emulator-5554"]}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![a]);
    assert!(b.card(other)["waiting"].as_str().unwrap().contains("emulator-5554"));

    let (code, msg) = b.post_err(&format!("tasks/T{a}"), json!({"locks": ["no spaces!"]}));
    assert_eq!(code, 400);
    assert!(msg.contains("can't be a lock name"), "{msg}");
    assert_eq!(b.post(&format!("tasks/T{a}"), json!({"locks": "none"}))["locks"], json!([]));
}

#[test]
fn alone_waits_for_the_goal_to_be_quiet_and_then_holds_it() {
    let b = new_board();
    let g = b.side_by_side();
    let ts: Vec<i64> = (1..=3).map(|i| b.task(&format!("Fix {i}"), json!({"goal_id": g}))).collect();
    runner::start_queued(&b.app).unwrap();
    let d = b.task("Measure", json!({"goal_id": g}));
    let e = b.task("Later", json!({"goal_id": g}));
    b.post(&format!("tasks/T{d}"), json!({"alone": true}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.card(d)["waiting"], format!("Waits to run alone (T{}, T{} and T{} are working)", ts[0], ts[1], ts[2]));
    assert_eq!(b.card(e)["waiting"], format!("Waits for T{d} to run alone first"));

    for (t, s) in ts.iter().zip(["s1", "s2", "s3"]) {
        b.run_to_done(*t, s);
    }
    runner::start_queued(&b.app).unwrap();
    assert!(b.started().contains(&d));
    assert!(!b.started().contains(&e));
    assert_eq!(b.card(e)["waiting"], format!("Waits while T{d} runs alone"));
    assert!(b.start_job(d).st("prompt").contains("runs alone in its goal"));
}

#[test]
fn alone_on_the_board_stops_everything_else_and_alone_in_a_goal_doesnt() {
    let b = new_board();
    let (g1, g2) = (b.side_by_side(), b.side_by_side());
    let a = b.task("Measure", json!({"goal_id": g1, "alone": "goal"}));
    let other = b.task("Elsewhere", json!({"goal_id": g2}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![a, other], "alone in its goal leaves other goals running");

    let b = new_board();
    let (g1, g2) = (b.side_by_side(), b.side_by_side());
    let a = b.task("Measure", json!({"goal_id": g1, "alone": "board"}));
    let other = b.task("Elsewhere", json!({"goal_id": g2}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![a]);
    assert_eq!(b.card(other)["waiting"], format!("Waits while T{a} runs alone"));
    assert_eq!(b.get("locks")["alone"][0]["scope"], "board");
    assert!(b.post(&format!("tasks/T{a}"), json!({"alone": "none"}))["alone"].is_null());
}

#[test]
fn propose_names_what_each_task_waits_for_by_ref_or_place() {
    let b = new_board();
    b.add_session("s1");
    let g = b.side_by_side();
    let setup = b.task("Set up", json!({}));
    let out = b.report("tb.propose", "s1", json!({"goal": format!("G{g}"), "tasks": [
        {"title": "Measure", "detail": "before numbers", "waits_for": [format!("T{setup}")], "locks": ["quiet-mac"]},
        "Fix the tabs::the fix::2::#1",
        {"title": "Measure after", "detail": "after", "waits_for": ["#1", "#2"], "alone": "goal"},
        "Docs::write them::::T1",
    ]}));
    let made: Vec<String> = out["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    let card = |r: &str| b.get(&format!("tasks/{r}"));
    assert_eq!(card(&made[0])["waits_for"], json!([format!("T{setup}")]));
    assert_eq!(card(&made[0])["locks"], json!(["quiet-mac"]));
    assert_eq!((card(&made[1])["waits_for"].clone(), card(&made[1])["wave"].clone()), (json!([made[0]]), json!(2)));
    assert_eq!(card(&made[1])["detail"], "the fix");
    assert_eq!(card(&made[2])["waits_for"], json!([made[0], made[1]]));
    assert_eq!(card(&made[2])["alone"], "goal");
    assert_eq!(card(&made[3])["waits_for"], json!(["T1"]));
    assert!(card(&made[3])["wave"].is_null());
    assert!(out["warnings"][0].as_str().unwrap().contains("quiet-mac"));
    let shown = b.get(&format!("goals/G{g}"));
    let two = shown["tasks"].as_array().unwrap().iter().find(|t| t["ref"] == made[1].as_str()).unwrap().clone();
    assert_eq!(two["waits_for_state"], json!([{"ref": made[0], "done": false}]));

    let bad = b.try_report("tb.propose", "s1", json!({"goal": format!("G{g}"), "tasks": [{"title": "First", "detail": "x", "waits_for": ["#1"]}]}));
    let (code, msg) = bad.unwrap_err();
    assert_eq!(code, 400);
    assert!(msg.contains("#1 is the first"), "{msg}");
}

fn git(path: &Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(path).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// The board's project folder as a clone of a bare origin with one commit on main.
fn git_repo(b: &Board) -> String {
    let origin = b.dir.path().join("origin.git");
    let repo = b.dir.path().join("webapp");
    std::fs::remove_dir_all(&repo).unwrap();
    git(b.dir.path(), &["init", "-q", "--bare", "-b", "main", &origin.to_string_lossy()]);
    git(b.dir.path(), &["clone", "-q", &origin.to_string_lossy(), &repo.to_string_lossy()]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["switch", "-q", "-c", "main"]);
    std::fs::write(repo.join("README"), "webapp\n").unwrap();
    git(&repo, &["add", "README"]);
    git(&repo, &["commit", "-q", "-m", "First"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["rev-parse", "HEAD"])
}

#[test]
fn each_task_starts_in_its_own_worktree_off_the_base() {
    let b = new_board();
    let head = git_repo(&b);
    let g = b.side_by_side();
    let plain = b.task("Plain", json!({}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.start_job(plain).st("cwd"), b.repo(), "without a base, tasks start in the shared folder");

    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    assert_eq!(b.get(&format!("goals/G{g}"))["worktree_base"], "origin/main");
    let (one, two) = (b.task("Fix one", json!({"goal_id": g})), b.task("Fix two", json!({"goal_id": g})));
    runner::start_queued(&b.app).unwrap();
    for t in [one, two] {
        let folder = format!("{}/.claude/worktrees/T{t}", b.repo());
        let job = b.start_job(t);
        assert_eq!(job.st("cwd"), folder);
        assert_eq!(git(Path::new(&folder), &["rev-parse", "HEAD"]), head);
        assert!(job.st("prompt").contains(&format!("The board made this worktree for the task: {folder}, detached at origin/main")), "{}", job.st("prompt"));
        let log = b.get(&format!("tasks/T{t}"))["log"].to_string();
        assert!(log.contains(&format!("Made its worktree from origin/main at {folder}")), "{log}");
    }
    let again = worktrees::ensure(&b.app, &board::get_task(&b.app, one).unwrap()).unwrap();
    assert_eq!(again, Some(format!("{}/.claude/worktrees/T{one}", b.repo())), "it reuses the worktree");
}

#[test]
fn a_worktree_base_is_checked_and_a_missing_one_fails_the_start_with_retries() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    let (code, msg) = b.post_err(&format!("goals/G{g}"), json!({"worktree_base": "a..b"}));
    assert_eq!(code, 400);
    assert!(msg.contains("isn't a branch"), "{msg}");
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "off"}));
    assert!(b.get(&format!("goals/G{g}"))["worktree_base"].is_null());

    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/nope"}));
    let t = b.task("Fix", json!({"goal_id": g}));
    runner::start_queued(&b.app).unwrap();
    assert!(b.started().is_empty());
    let row = board::get_task(&b.app, t).unwrap();
    assert!(row.s("retry_at").is_some(), "it tries again later");
    assert!(row.st("latest").contains("Couldn't make its worktree from origin/nope"), "{}", row.st("latest"));
    runner::start_queued(&b.app).unwrap();
    assert!(b.started().is_empty(), "it waits for the retry");
}

#[test]
fn a_finished_clean_worktree_is_removed_and_a_dirty_one_kept() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let (clean, dirty, working) = (b.task("Clean", json!({"goal_id": g})), b.task("Dirty", json!({"goal_id": g})), b.task("Working", json!({"goal_id": g})));
    runner::start_queued(&b.app).unwrap();
    let folder = |t: i64| format!("{}/.claude/worktrees/T{t}", b.repo());
    std::fs::write(Path::new(&folder(dirty)).join("left"), "x\n").unwrap();
    for t in [clean, dirty] {
        b.app.db.x("UPDATE tasks SET status = 'done', session_id = NULL WHERE id = ?", p![t]).unwrap();
    }
    assert_eq!(worktrees::clean_up(&b.app).unwrap(), vec![clean]);
    assert!(!Path::new(&folder(clean)).exists());
    assert!(Path::new(&folder(dirty)).exists());
    assert!(Path::new(&folder(working)).exists());
    assert!(b.card(clean)["log"].to_string().contains(&format!("Removed its worktree at {}", folder(clean))));
    assert!(b.card(dirty)["log"].to_string().contains(&format!("Left its worktree at {}: it has uncommitted changes", folder(dirty))));
    assert!(worktrees::clean_up(&b.app).unwrap().is_empty(), "each is looked at once");
}

#[test]
fn a_hook_from_another_tasks_worktree_leaves_the_task_where_it_was() {
    let b = new_board();
    let wt = |n: &str| format!("{}/.claude/worktrees/{n}", b.repo());
    b.add_session("s1");
    b.add_session("s2");
    let a = b.task("Land the test", json!({"pickup": "manual"}));
    b.report("hook.prompt", "s1", json!({"prompt": format!("[task-board:T{a}] go")}));
    b.report("hook.stop", "s1", json!({"cwd": wt("ct-T1"), "last_message": "Built it.", "git": {"branch": "chore/tuple", "commit": "x", "sha": "y", "uncommitted": 0}}));
    let t2 = b.task("Build on it", json!({"pickup": "manual"}));
    b.report("hook.prompt", "s2", json!({"prompt": format!("[task-board:T{t2}] go")}));
    b.report("hook.stop", "s2", json!({"cwd": format!("{}/app", wt("ct-T2")), "last_message": "Built it.", "git": {"branch": "feature/build-on-it", "commit": "Add it", "sha": "abc", "uncommitted": 0}}));
    b.report("hook.pre_compact", "s2", json!({"cwd": wt("ct-T1"), "git": {"branch": "chore/tuple", "commit": "x", "sha": "y", "uncommitted": 0}}));
    let w = &b.card(t2)["context"]["where"];
    assert_eq!((w["branch"].as_str().unwrap(), w["worktree"].as_str().unwrap()), ("feature/build-on-it", format!("{}/app", wt("ct-T2")).as_str()));
}

#[test]
fn a_pr_another_task_owns_is_never_linked_again() {
    let b = new_board();
    let (a, t2) = (b.task("Land the test", json!({})), b.task("Build on it", json!({})));
    let pr = PrLink { host: "github".into(), repo: "acme/webapp".into(), num: 15, url: "https://github.com/acme/webapp/pull/15".into() };
    assert!(prflow::link_pr(&b.app, &board::get_task(&b.app, a).unwrap(), &pr, "T1").unwrap());
    assert!(!prflow::link_pr(&b.app, &board::get_task(&b.app, t2).unwrap(), &pr, "T2").unwrap());
    assert!(b.card(t2)["pr"].is_null());
}

fn write_turn(b: &Board, sid: &str, prompt: &str, files: &[&str], at: &str) {
    let s = board::get_session(&b.app, Some(sid)).unwrap().unwrap();
    let file = taskboardd::transcript::transcript_file(&b.app.cfg.claude_projects, &s.st("project_path"), &s.st("claude_session_id"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut lines = vec![json!({"type": "user", "message": {"content": prompt}, "timestamp": at})];
    for f in files {
        lines.push(json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Edit", "input": {"file_path": format!("{}/{f}", b.repo())}}]}}));
    }
    std::fs::write(&file, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
}

#[test]
fn more_changes_after_done_reopen_the_task() {
    let b = new_board();
    b.add_session("s1");
    b.report("hook.prompt", "s1", json!({"prompt": "zip the project"}));
    let tr = b.report("tb.new_task", "s1", json!({"title": "Zip the project", "here": true, "ships_pr": false}))["created"][0].as_str().unwrap().to_string();
    write_turn(&b, "s1", "zip the project", &["notes.md"], "2026-01-01T10:00:00Z");
    b.report("tb.done", "s1", json!({"summary": "Zipped it."}));
    assert!(b.report("hook.stop", "s1", json!({"last_message": "Zipped."})).get("block").is_none());
    assert_eq!(b.get(&format!("tasks/{tr}"))["status"], "done");

    b.report("hook.prompt", "s1", json!({"prompt": "did my tab close?"}));
    write_turn(&b, "s1", "did my tab close?", &[], "2099-01-01T00:00:00Z");
    assert!(b.report("hook.stop", "s1", json!({"last_message": "No."})).get("block").is_none());
    assert_eq!(b.get(&format!("tasks/{tr}"))["status"], "done", "a question doesn't reopen it");

    b.report("hook.prompt", "s1", json!({"prompt": "add the change list too"}));
    write_turn(&b, "s1", "add the change list too", &["changes.md"], "2099-01-01T00:01:00Z");
    let out = b.report("hook.stop", "s1", json!({"last_message": "Added it."}));
    let block = out["block"].as_str().expect("the stop is blocked");
    assert!(block.contains(&format!("after {tr} was marked done, so the board reopened it")), "{block}");
    let t = b.get(&format!("tasks/{tr}"));
    assert_eq!((t["status"].as_str().unwrap(), t["finished_at"].is_null()), ("working", true));
    assert_eq!(b.report("tb.status", "s1", json!({}))["task"], tr.as_str());
    b.report("tb.done", "s1", json!({"summary": "Zipped it and wrote the change list."}));
    let t = b.get(&format!("tasks/{tr}"));
    assert_eq!((t["status"].as_str().unwrap(), t["summary"].as_str().unwrap()), ("done", "Zipped it and wrote the change list."));
}

#[test]
fn more_changes_leave_a_done_pr_task_to_the_pr_flow() {
    let b = new_board();
    b.add_session("s1");
    let tr = b.report("tb.new_task", "s1", json!({"title": "Fix the tab bar", "here": true}))["created"][0].as_str().unwrap().to_string();
    b.report("tb.done", "s1", json!({"summary": "Fixed it.", "pr": "https://github.com/acme/webapp/pull/7"}));
    b.report("hook.prompt", "s1", json!({"prompt": "one more tweak"}));
    write_turn(&b, "s1", "one more tweak", &["TabBar.swift"], "2099-01-01T00:00:00Z");
    b.report("hook.stop", "s1", json!({"last_message": "Tweaked."}));
    assert_eq!(b.get(&format!("tasks/{tr}"))["status"], "done");
}

#[test]
fn done_leaves_a_tab_the_owner_opened() {
    let b = new_board();
    b.add_session("s1");
    let t = b.task("Zip it", json!({"pickup": "manual"}));
    b.report("hook.prompt", "s1", json!({"prompt": format!("[task-board:T{t}] go")}));
    b.report("tb.done", "s1", json!({"summary": "Zipped it."}));
    b.report("hook.stop", "s1", json!({"last_message": "All done."}));
    b.app.db.x("UPDATE sessions SET status = 'idle', status_at = '2000-01-01T00:00:00Z' WHERE id = 's1'", p![]).unwrap();
    runner::auto_close_done(&b.app).unwrap();
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM jobs WHERE kind = 'close'", p![]).unwrap(), 0);

    b.open_for("s1", t);
    runner::auto_close_done(&b.app).unwrap();
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM jobs WHERE kind = 'close'", p![]).unwrap(), 1, "a tab the board opened closes");
}
