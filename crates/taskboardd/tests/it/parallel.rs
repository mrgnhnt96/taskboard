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
use taskboardd::{board, midna, p, prflow, progress, reports, runner, slot_ageing, worklane, worktrees};

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
fn each_task_starts_in_a_worktree_slot_off_the_base() {
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
    for (n, t) in [(1, one), (2, two)] {
        let folder = slot(&b, n);
        let job = b.start_job(t);
        assert_eq!(job.st("cwd"), folder);
        assert_eq!(git(Path::new(&folder), &["rev-parse", "HEAD"]), head);
        assert!(job.st("prompt").contains(&format!("The board gave the task the worktree slot-{n} from its pool: {folder}, detached at origin/main")), "{}", job.st("prompt"));
        assert!(job.st("prompt").contains(&format!("Its builds go in {folder}/target")), "{}", job.st("prompt"));
        let log = b.get(&format!("tasks/T{t}"))["log"].to_string();
        assert!(log.contains(&format!("Made worktree slot-{n} from origin/main at {folder}")), "{log}");
    }
    let again = worktrees::ensure(&b.app, &board::get_task(&b.app, one).unwrap()).unwrap();
    assert_eq!(again, Some(slot(&b, 1)), "it keeps its slot");
}

#[test]
fn making_a_worktree_says_each_stage_on_the_progress_stream() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let t = b.task("Fix one", json!({"goal_id": g}));
    let lines = Arc::new(parking_lot::Mutex::new(vec![]));
    let l = lines.clone();
    let sink: progress::Sink = Arc::new(move |s: &str| l.lock().push(s.to_string()));
    progress::with(sink, || worktrees::ensure(&b.app, &board::get_task(&b.app, t).unwrap())).unwrap();
    assert_eq!(*lines.lock(), ["Fetched origin", "Made slot-1 from origin/main"]);
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

fn slot(b: &Board, n: i64) -> String {
    format!("{}/.claude/worktrees/slot-{n}", b.repo())
}

/// A worktree of the task's own, as the board made them before the slot pool.
fn own_worktree(b: &Board, t: i64) {
    let folder = format!("{}/.claude/worktrees/T{t}", b.repo());
    git(Path::new(&b.repo()), &["worktree", "add", "-q", "--detach", &folder, "origin/main"]);
}

/// Marks a task done with its terminal gone, which frees its slot.
fn finish(b: &Board, t: i64) {
    b.app.db.x("UPDATE tasks SET status = 'done', session_id = NULL WHERE id = ?", p![t]).unwrap();
}

#[test]
fn a_task_takes_the_free_slot_used_last_and_passes_over_a_dirty_one() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let (one, two) = (b.task("One", json!({"goal_id": g})), b.task("Two", json!({"goal_id": g})));
    runner::start_queued(&b.app).unwrap();
    assert_eq!((b.start_job(one).st("cwd"), b.start_job(two).st("cwd")), (slot(&b, 1), slot(&b, 2)));
    let third = b.task("Third", json!({"goal_id": g}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.start_job(third).st("cwd"), slot(&b, 3), "a held slot isn't shared");

    git(Path::new(&slot(&b, 2)), &["switch", "-q", "-c", "feature/two"]);
    for t in [one, two] {
        finish(&b, t);
    }
    assert!(worktrees::clean_up(&b.app).unwrap().is_empty(), "a slot stays for the next task");
    let next = b.task("Next", json!({"goal_id": g}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.start_job(next).st("cwd"), slot(&b, 2), "the warmest free slot");
    assert!(git(Path::new(&slot(&b, 2)), &["status", "--short", "--branch"]).starts_with("## HEAD (no branch)"), "detached at the base again");
    assert!(b.card(next)["log"].to_string().contains(&format!("Took worktree slot-2 at {}, detached at origin/main", slot(&b, 2))));

    finish(&b, next);
    std::fs::write(Path::new(&slot(&b, 2)).join("left"), "x\n").unwrap();
    let after = b.task("After", json!({"goal_id": g}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.start_job(after).st("cwd"), slot(&b, 1), "a slot with uncommitted changes is passed over");
    assert!(Path::new(&slot(&b, 2)).join("left").exists());
}

#[test]
fn at_max_slots_a_task_waits_for_a_slot_like_a_lock() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let out = b.post("projects/webapp", json!({"min_slots": 1, "max_slots": 1}));
    assert_eq!(out["worktrees"]["max_slots"], 1);
    let (one, two) = (b.task("One", json!({"goal_id": g})), b.task("Two", json!({"goal_id": g})));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![one]);
    assert_eq!(b.card(two)["waiting"], "Waits for a worktree slot");
    let locks = b.get("locks")["locks"].clone();
    assert_eq!((locks[0]["name"].clone(), locks[0]["held_by"].clone()), (json!("wt:slot-1"), json!(format!("T{one}"))), "{locks}");

    finish(&b, one);
    runner::start_queued(&b.app).unwrap();
    assert!(b.started().contains(&two));
    assert_eq!(b.start_job(two).st("cwd"), slot(&b, 1));
}

#[test]
fn a_project_s_slot_settings_are_checked_and_go_back_to_config() {
    let b = new_board();
    let w = |out: &Value| out["worktrees"].clone();
    let out = b.get("projects")["projects"][0].clone();
    assert_eq!(w(&out), json!({"min_slots": 2, "max_slots": 10, "idle_trim_days": 2, "idle_remove_days": 7, "low_disk_gb": 50, "set": {}}));
    let out = b.post("projects/webapp", json!({"min_slots": "1", "idle_trim_days": 3, "low_disk_gb": 20}));
    assert_eq!(w(&out)["set"], json!({"min_slots": 1, "idle_trim_days": 3, "low_disk_gb": 20}));
    let (code, msg) = b.post_err("projects/webapp", json!({"max_slots": 0}));
    assert_eq!(code, 400);
    assert!(msg.contains("max_slots is a count from 1 to 50"), "{msg}");
    let (_, msg) = b.post_err("projects/webapp", json!({"idle_remove_days": 2}));
    assert!(msg.contains("idle_trim_days (3) can't be more than idle_remove_days (2)"), "{msg}");
    assert_eq!(b.get("projects")["projects"][0]["worktrees"]["idle_remove_days"], 7, "a refused change isn't kept");
    let out = b.post("projects/webapp", json!({"min_slots": "default", "idle_trim_days": null, "low_disk_gb": null}));
    assert_eq!(w(&out)["set"], json!({}));
}

#[test]
fn a_finished_clean_worktree_is_removed_and_a_dirty_one_kept() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let (clean, dirty, working) = (b.task("Clean", json!({"goal_id": g})), b.task("Dirty", json!({"goal_id": g})), b.task("Working", json!({"goal_id": g})));
    for t in [clean, dirty, working] {
        own_worktree(&b, t);
    }
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
fn a_pr_task_keeps_its_worktree_until_its_pr_is_linked_and_finished() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let (waiting, canceled, merged) =
        (b.task("No PR yet", json!({"goal_id": g, "ships_pr": true})), b.task("Canceled", json!({"goal_id": g, "ships_pr": true})), b.task("Merged", json!({"goal_id": g, "ships_pr": true})));
    for t in [waiting, canceled, merged] {
        own_worktree(&b, t);
    }
    runner::start_queued(&b.app).unwrap();
    for t in [waiting, canceled, merged] {
        b.app.db.x("UPDATE tasks SET status = 'done', session_id = NULL WHERE id = ?", p![t]).unwrap();
    }
    b.app.db.x("UPDATE tasks SET no_pr = 'nothing to change' WHERE id = ?", p![canceled]).unwrap();
    b.app.db.x("UPDATE tasks SET pr_num = 3, pr_repo = 'acme/webapp', pr_state = 'MERGED', pr_phase = 'merged' WHERE id = ?", p![merged]).unwrap();
    assert_eq!(worktrees::clean_up(&b.app).unwrap(), vec![canceled, merged], "a PR task with no PR yet keeps its worktree");
    assert!(Path::new(&format!("{}/.claude/worktrees/T{waiting}", b.repo())).exists());

    b.app.db.x("UPDATE tasks SET pr_num = 4, pr_repo = 'acme/webapp', pr_state = 'OPEN', pr_phase = 'review' WHERE id = ?", p![waiting]).unwrap();
    assert!(worktrees::clean_up(&b.app).unwrap().is_empty(), "nor while its PR is open");
    b.app.db.x("UPDATE tasks SET pr_state = 'CLOSED', pr_phase = 'declined' WHERE id = ?", p![waiting]).unwrap();
    assert_eq!(worktrees::clean_up(&b.app).unwrap(), vec![waiting]);
}

#[test]
fn worktrees_agents_made_are_matched_to_their_task_and_removed_by_the_same_rules() {
    let b = new_board();
    git_repo(&b);
    let repo = b.repo();
    let dir = b.dir.path().to_string_lossy().to_string();
    let add = |path: &str, branch: Option<&str>| match branch {
        Some(br) => git(Path::new(&repo), &["worktree", "add", "-q", "-b", br, path]),
        None => git(Path::new(&repo), &["worktree", "add", "-q", "--detach", path]),
    };
    let [by_branch, by_folder, by_where, active, dirty, pr_open] = ["By branch", "By folder", "By where", "Active", "Dirty", "PR open"].map(|t| b.task(t, json!({})));
    let wt = |n: &str| format!("{dir}/{n}");
    add(&wt("webapp-fix"), Some(&format!("feat/t{by_branch}-thing")));
    add(&wt(&format!("webapp-t{by_folder}")), None);
    add(&wt("elsewhere"), Some("misc"));
    add(&wt(&format!("webapp-t{active}")), None);
    add(&wt(&format!("webapp-t{dirty}")), None);
    add(&wt(&format!("webapp-t{pr_open}")), None);
    add(&wt("scratch"), Some("spike"));
    std::fs::write(Path::new(&wt(&format!("webapp-t{dirty}"))).join("left"), "x\n").unwrap();
    let ctx = json!({"where": {"worktree": format!("{}/sub", wt("elsewhere"))}}).to_string();
    b.app.db.x("UPDATE tasks SET context = ? WHERE id = ?", p![ctx, by_where]).unwrap();
    for t in [by_branch, by_folder, by_where, dirty, pr_open] {
        b.app.db.x("UPDATE tasks SET status = 'done', session_id = NULL WHERE id = ?", p![t]).unwrap();
    }
    b.app.db.x("UPDATE tasks SET pr_num = 4, pr_repo = 'acme/webapp', pr_state = 'OPEN', pr_phase = 'review' WHERE id = ?", p![pr_open]).unwrap();

    let mut removed = worktrees::clean_up(&b.app).unwrap();
    removed.sort();
    assert_eq!(removed, vec![by_branch, by_folder, by_where]);
    for gone in [wt("webapp-fix"), wt(&format!("webapp-t{by_folder}")), wt("elsewhere")] {
        assert!(!Path::new(&gone).exists(), "{gone}");
    }
    for kept in [wt(&format!("webapp-t{active}")), wt(&format!("webapp-t{dirty}")), wt(&format!("webapp-t{pr_open}")), wt("scratch"), repo.clone()] {
        assert!(Path::new(&kept).exists(), "{kept}");
    }
    assert_eq!(git(Path::new(&repo), &["worktree", "list"]).lines().count(), 5, "the main checkout and the four kept");
    let log = b.card(by_branch)["log"].to_string();
    assert!(log.contains("Removed its worktree at ") && log.contains("/webapp-fix"), "{log}");
    let dirty_log = b.card(dirty)["log"].to_string();
    assert!(dirty_log.contains("it has uncommitted changes"), "{dirty_log}");
    assert!(b.card(pr_open)["log"].to_string().contains("its PR is still open"));
    assert!(!b.card(active)["log"].to_string().contains("Left its worktree"), "an active task's timeline stays quiet");
    let stray = &b.get("projects")["projects"][0]["stray_worktrees"];
    assert_eq!(stray.as_array().unwrap().len(), 1, "{stray}");
    assert!(stray[0].as_str().unwrap().ends_with("/scratch"), "{stray}");

    assert!(worktrees::clean_up(&b.app).unwrap().is_empty());
    assert_eq!(b.card(dirty)["log"].to_string().matches("it has uncommitted changes").count(), dirty_log.matches("it has uncommitted changes").count(), "it says why once");
    b.app.db.x("UPDATE tasks SET pr_state = 'MERGED', pr_phase = 'merged' WHERE id = ?", p![pr_open]).unwrap();
    assert_eq!(worktrees::clean_up(&b.app).unwrap(), vec![pr_open], "once its PR is merged");
}

#[test]
fn a_stacked_task_starts_from_its_parent_s_branch_where_it_exists() {
    let b = new_board();
    git_repo(&b);
    let repo = b.repo();
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let parent = b.task("Parent", json!({}));
    b.app.db.x("UPDATE tasks SET status = 'done', context = '{\"where\": {\"branch\": \"feature/parent\"}}' WHERE id = ?", p![parent]).unwrap();
    let start_of = |title: &str| {
        let t = b.task(title, json!({"goal_id": g}));
        b.app.db.x("UPDATE tasks SET pr_after = ? WHERE id = ?", p![parent, t]).unwrap();
        worktrees::ensure(&b.app, &board::get_task(&b.app, t).unwrap()).unwrap();
        board::task_context(&board::get_task(&b.app, t).unwrap()).s("worktree_base").unwrap_or("").to_string()
    };
    assert_eq!(start_of("Ahead of its parent"), "origin/main", "a parent branch that exists nowhere falls back to the base");
    git(Path::new(&repo), &["branch", "feature/parent"]);
    assert_eq!(start_of("Parent not pushed"), "feature/parent", "then the local branch");
    git(Path::new(&repo), &["push", "-q", "origin", "feature/parent"]);
    assert_eq!(start_of("Parent pushed"), "origin/feature/parent", "then the remote one");
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

#[test]
fn a_merged_pr_closes_a_board_tab_that_never_became_the_task_s_terminal() {
    let b = new_board();
    b.add_session("s1");
    let t = b.task("Fix the tab bar", json!({"ships_pr": true}));
    b.report("tb.take", "s1", json!({"task": format!("T{t}")}));
    b.report("tb.done", "s1", json!({"summary": "Fixed it.", "pr": "https://github.com/acme/webapp/pull/7"}));
    b.app.db.x("UPDATE tasks SET pr_phase = 'merged', pr_state = 'MERGED' WHERE id = ?", p![t]).unwrap();
    for sid in ["S-LOST", "S-OWN"] {
        midna::sync(&b.app, &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": b.repo(), "status": {"state": "idle"}})], &[]).unwrap();
    }
    b.app.db.x("UPDATE sessions SET status = 'idle', status_at = '2000-01-01T00:00:00Z' WHERE id IN ('S-LOST', 'S-OWN')", p![]).unwrap();
    b.open_for("S-LOST", t);
    b.app.db.x("INSERT INTO task_terminals(task_id, session_id, why, at) VALUES (?, 'S-OWN', 'owner', '2000-01-01T00:00:00Z')", p![t]).unwrap();
    let closing: Vec<String> = prflow::pr_tabs_to_close(&b.app).unwrap().into_iter().map(|(s, _, _)| s.st("id")).collect();
    assert_eq!(closing, vec!["S-LOST".to_string()], "the board's lost tab closes; the owner's stays");
}

#[test]
fn only_a_task_that_ships_a_pr_picks_one_up_from_a_stop_or_a_push() {
    let b = new_board();
    for (sid, ships, num) in [("s1", false, 7), ("s2", true, 8)] {
        b.add_session(sid);
        let t = b.task(&format!("Task {sid}"), json!({"ships_pr": ships}));
        b.report("tb.take", sid, json!({"task": format!("T{t}")}));
        b.report("hook.stop", sid, json!({"last_message": format!("Looked at https://github.com/acme/webapp/pull/{num}, the race is there.")}));
        let stop_pr = board::get_task(&b.app, t).unwrap().i("pr_num");
        b.app.db.x("UPDATE tasks SET pr_num = NULL, pr_url = NULL, pr_repo = NULL WHERE id = ?", p![t]).unwrap();
        b.report("hook.commit", sid, json!({"command": "git push -u origin feat", "output": format!("remote: View pull request: https://github.com/acme/webapp/pull/{num}")}));
        let push_pr = board::get_task(&b.app, t).unwrap().i("pr_num");
        let want = ships.then_some(num);
        assert_eq!((stop_pr, push_pr), (want, want), "ships_pr = {ships}");
    }
}

#[test]
fn taking_back_a_pr_task_drops_the_owner_s_review_until_a_peer_is_asked() {
    let b = new_board();
    let flow = |t: i64| -> Value { serde_json::from_str(board::get_task(&b.app, t).unwrap().s("pr_flow").unwrap_or("{}")).unwrap() };
    let green = json!({"state": "OPEN", "head": "h2", "checks": [{"name": "ci", "state": "passed"}], "failed": [], "running": 0, "comments": 0, "approvals": 0, "review_decision": ""});
    for (sid, peers) in [("s1", false), ("s2", true)] {
        b.add_session(sid);
        let t = b.task(&format!("Fix {sid}"), json!({"ships_pr": true}));
        b.report("tb.take", sid, json!({"task": format!("T{t}")}));
        b.report("tb.done", sid, json!({"summary": "Fixed it.", "pr": format!("https://github.com/acme/webapp/pull/{}", if peers { 12 } else { 11 })}));
        let mut marks = vec![("reviewed", json!("2026-10-12T10:00:00Z")), ("review_alerted", json!("2026-10-12T09:00:00Z"))];
        if peers {
            marks.push(("asked", json!({"at": "2026-10-12T10:05:00Z", "names": ["Pat"], "by": "board"})));
        }
        prflow::merge_flow(&b.app, t, marks).unwrap();
        b.report("tb.take", sid, json!({"task": format!("T{t}")}));
        let f = flow(t);
        if peers {
            assert!(f.get("reviewed").is_some() && f.get("asked").is_some(), "peers are on it, so it goes to re-review as it is");
            continue;
        }
        assert!(f.get("reviewed").is_none() && f.get("review_alerted").is_none(), "{f}");
        b.report("tb.done", sid, json!({"summary": "Moved the fix to the shared widget."}));
        b.app.db.tx(|| prflow::step(&b.app, &board::get_task(&b.app, t).unwrap(), &green)).unwrap();
        assert_eq!(board::get_task(&b.app, t).unwrap().s("pr_phase"), Some("review"));
        let alerts = taskboardd::dispatch::alerts(&b.app);
        assert!(alerts.iter().any(|a| a["text"].as_str().unwrap().contains(&format!("T{t} is green and ready for review"))), "{alerts:?}");
    }
}

#[test]
fn closing_finished_tasks_terminals_leaves_a_working_one_open() {
    let b = new_board();
    let mut done = vec![];
    for sid in ["s1", "s2", "s3"] {
        b.add_session(sid);
        let t = b.task(&format!("Task {sid}"), json!({"pickup": "manual"}));
        b.report("tb.take", sid, json!({"task": format!("T{t}")}));
        if sid != "s3" {
            b.report("tb.done", sid, json!({"summary": "Done."}));
            done.push(sid);
        }
    }
    b.app.db.x("UPDATE sessions SET status = 'idle', status_at = '2000-01-01T00:00:00Z'", p![]).unwrap();
    assert_eq!(b.post("done/close-terminals", json!({}))["jobs"], 2);
    let closing: Vec<String> = b
        .app
        .db
        .q("SELECT args FROM jobs WHERE kind = 'close' ORDER BY id", p![])
        .unwrap()
        .iter()
        .filter_map(|j| board::job_args(j).s("session").map(|s| s.to_string()))
        .collect();
    assert_eq!(closing, done, "the working task's terminal stays open");
}

#[test]
fn propose_with_no_titled_task_is_refused() {
    let b = new_board();
    b.add_session("s1");
    let g = b.side_by_side();
    for tasks in [json!([]), json!(["::only a detail"])] {
        let (code, msg) = b.try_report("tb.propose", "s1", json!({"goal": format!("G{g}"), "tasks": tasks})).unwrap_err();
        assert_eq!(code, 400);
        assert!(msg.contains("at least one task with a title"), "{msg}");
    }
    assert!(b.get(&format!("goals/G{g}"))["tasks"].as_array().unwrap().is_empty());
}

#[test]
fn the_no_task_ask_names_no_pr_for_work_that_won_t_end_in_one() {
    let b = new_board();
    b.add_session("s1");
    b.report("hook.prompt", "s1", json!({"prompt": "tweak the config"}));
    write_turn(&b, "s1", "tweak the config", &["config.toml"], "2099-01-01T00:00:00Z");
    let out = b.report("hook.stop", "s1", json!({"last_message": "Tweaked."}));
    let block = out["block"].as_str().expect("the stop is blocked");
    assert!(block.contains("--here (add --no-pr if it won't end in a PR)"), "{block}");
}

#[test]
fn idle_slots_lose_their_build_output_then_the_checkout_but_never_below_min() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let ts: Vec<i64> = (1..=4).map(|i| b.task(&format!("T{i}"), json!({"goal_id": g}))).collect();
    runner::start_queued(&b.app).unwrap();
    for n in 1..=4 {
        std::fs::create_dir_all(Path::new(&slot(&b, n)).join("target")).unwrap();
        std::fs::write(Path::new(&slot(&b, n)).join("target/o"), "x").unwrap();
    }
    for t in &ts[..3] {
        finish(&b, *t);
    }
    let day = 86_400.0;
    let now = taskboardd::util::now_ts() + 10.0 * day;
    std::fs::write(Path::new(&slot(&b, 3)).join("left"), "x\n").unwrap();
    let removed = slot_ageing::sweep_at(&b.app, now, &|_| None).unwrap();
    assert_eq!(removed, [slot(&b, 1), slot(&b, 2)], "slot-3 is dirty and slot-4 is claimed, so they stay");
    assert!(!Path::new(&slot(&b, 1)).exists() && !Path::new(&slot(&b, 2)).exists());
    assert!(Path::new(&slot(&b, 3)).join("target").exists(), "uncommitted changes: untouched");
    assert!(Path::new(&slot(&b, 4)).join("target").exists(), "claimed: untouched");

    let kept = slot_ageing::sweep_at(&b.app, now + 30.0 * day, &|_| None).unwrap();
    assert!(kept.is_empty(), "the pool never drops below min_slots");
    assert!(Path::new(&slot(&b, 3)).is_dir());
    std::fs::remove_file(Path::new(&slot(&b, 3)).join("left")).unwrap();
    std::fs::create_dir_all(Path::new(&slot(&b, 3)).join("target")).unwrap();
    slot_ageing::sweep_at(&b.app, taskboardd::util::now_ts() + 1.0, &|_| Some(1.0)).unwrap();
    assert!(!Path::new(&slot(&b, 3)).join("target").exists(), "low disk clears a free slot before its timer");
    assert!(Path::new(&slot(&b, 4)).join("target").exists(), "still claimed");
}

/// A commit on a new branch in the slot, pushed or not.
fn commit_in(folder: &str, branch: &str, push: bool) {
    let dir = Path::new(folder);
    git(dir, &["switch", "-q", "-c", branch]);
    std::fs::write(dir.join("change"), "x\n").unwrap();
    git(dir, &["add", "change"]);
    git(dir, &["commit", "-q", "-m", "Change"]);
    if push {
        git(dir, &["push", "-q", "-u", "origin", branch]);
    }
}

fn slot_row(b: &Board, n: i64) -> taskboardd::util::Row {
    b.app.db.q1("SELECT * FROM worktree_slots WHERE n = ?", p![n]).unwrap().unwrap()
}

#[test]
fn submitting_gives_the_slot_back_detached_with_its_branch_kept() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    b.post("projects/webapp", json!({"min_slots": 1, "max_slots": 1}));
    let (one, two) = (b.task("One", json!({"goal_id": g})), b.task("Two", json!({"goal_id": g})));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![one]);
    let slots = b.get("slots")["slots"].clone();
    assert_eq!(slots[0]["name"], "slot-1");
    assert_eq!(slots[0]["held_by"], format!("T{one}"));
    assert_eq!(slots[0]["waiting"], json!([format!("T{two}")]), "{slots}");
    assert!(slots[0]["size_kb"].as_i64().is_some(), "{slots}");
    // used_at is whole seconds: backdate it so a claim and give-back in one second still differ.
    b.app.db.x("UPDATE worktree_slots SET used_at = '2000-01-01T00:00:00Z' WHERE n = 1", p![]).unwrap();
    let used = slot_row(&b, 1).s("used_at").map(|s| s.to_string());

    commit_in(&slot(&b, 1), "feature/one", true);
    b.run_to_done(one, "s1");
    let row = slot_row(&b, 1);
    assert!(row.i("task_id").is_none());
    assert_ne!(row.s("used_at").map(|s| s.to_string()), used, "its last use is recorded");
    assert!(git(Path::new(&slot(&b, 1)), &["status", "--short", "--branch"]).starts_with("## HEAD (no branch)"), "detached at the base again");
    let card = b.card(one);
    assert!(card["log"].to_string().contains("Gave back worktree slot-1; its branch feature/one is kept for review fixes"), "{}", card["log"]);
    let ctx = board::task_context(&board::get_task(&b.app, one).unwrap());
    assert_eq!(ctx.s("worktree_branch"), Some("feature/one"));
    assert!(b.get("slots")["slots"][0]["held_by"].is_null(), "free while its terminal is still open");

    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.start_job(two).st("cwd"), slot(&b, 1), "the next task takes it at once");
}

#[test]
fn a_slot_with_uncommitted_or_unpushed_work_is_not_given_back() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let one = b.task("One", json!({"goal_id": g}));
    runner::start_queued(&b.app).unwrap();
    b.add_session("s1");
    b.open_for("s1", one);
    b.report("hook.prompt", "s1", json!({"prompt": b.start_job(one).st("prompt")}));

    std::fs::write(Path::new(&slot(&b, 1)).join("loose"), "x\n").unwrap();
    let (code, msg) = b.try_report("tb.done", "s1", json!({"summary": "Done."})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("slot-1") && msg.contains("uncommitted changes. Commit and push them, or discard them, then run this again."), "{msg}");
    let (code, msg) = b.post_err(&format!("tasks/T{one}/slot/release"), json!({}));
    assert_eq!(code, 409, "{msg}");

    std::fs::remove_file(Path::new(&slot(&b, 1)).join("loose")).unwrap();
    commit_in(&slot(&b, 1), "feature/one", false);
    let (code, msg) = b.try_report("tb.done", "s1", json!({"summary": "Done."})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("1 commit on feature/one isn't pushed. Push it (git push), then run this again."), "{msg}");
    assert_eq!(board::get_task(&b.app, one).unwrap().s("status"), Some("working"), "nothing finished");

    git(Path::new(&slot(&b, 1)), &["push", "-q", "-u", "origin", "feature/one"]);
    b.report("tb.done", "s1", json!({"summary": "Done."}));
    assert!(slot_row(&b, 1).i("task_id").is_none());
}

#[test]
fn failing_or_parking_gives_the_slot_back_when_it_is_clean() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let (one, two) = (b.task("One", json!({"goal_id": g})), b.task("Two", json!({"goal_id": g})));
    runner::start_queued(&b.app).unwrap();
    let other = b.task("Needed", json!({}));
    for (t, sid) in [(one, "s1"), (two, "s2")] {
        b.add_session(sid);
        b.open_for(sid, t);
        b.report("hook.prompt", sid, json!({"prompt": b.start_job(t).st("prompt")}));
    }

    b.report("tb.wait_for", "s1", json!({"tasks": [format!("T{other}")], "why": "Needs it."}));
    assert!(slot_row(&b, 1).i("task_id").is_none(), "a parked task waits without a slot");

    std::fs::write(Path::new(&slot(&b, 2)).join("loose"), "x\n").unwrap();
    b.report("tb.fail", "s2", json!({"reason": "Can't."}));
    assert_eq!(slot_row(&b, 2).i("task_id"), Some(two), "failing keeps a slot with work only there");
    assert!(b.card(two)["log"].to_string().contains("Kept its worktree slot"), "{}", b.card(two)["log"]);
}

#[test]
fn review_fixes_reclaim_a_slot_on_the_pushed_branch() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    let one = b.task("One", json!({"goal_id": g}));
    runner::start_queued(&b.app).unwrap();
    commit_in(&slot(&b, 1), "feature/one", true);
    let head = git(Path::new(&slot(&b, 1)), &["rev-parse", "HEAD"]);
    b.run_to_done(one, "s1");

    let out = b.post(&format!("tasks/T{one}/slot/claim"), json!({}));
    assert_eq!((out["slot"].clone(), out["path"].clone()), (json!("slot-1"), json!(slot(&b, 1))), "its own slot while it's free");
    assert_eq!(git(Path::new(&slot(&b, 1)), &["rev-parse", "--abbrev-ref", "HEAD"]), "feature/one");
    assert_eq!(git(Path::new(&slot(&b, 1)), &["rev-parse", "HEAD"]), head);
    assert_eq!(b.post(&format!("tasks/T{one}/slot/claim"), json!({}))["slot"], "slot-1", "claiming again keeps it");
    assert_eq!(b.post(&format!("tasks/T{one}/slot/release"), json!({}))["released"], true);
    assert_eq!(b.post(&format!("tasks/T{one}/slot/release"), json!({}))["released"], false);

    let next = b.task("Next", json!({"goal_id": g}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.start_job(next).st("cwd"), slot(&b, 1));
    let out = b.post(&format!("tasks/T{one}/slot/claim"), json!({}));
    assert_eq!(out["slot"], "slot-2", "another slot while its own is taken");
    assert_eq!(git(Path::new(&slot(&b, 2)), &["rev-parse", "--abbrev-ref", "HEAD"]), "feature/one");
    assert!(b.card(one)["log"].to_string().contains(&format!("Made worktree slot-2 at {}, on its branch feature/one", slot(&b, 2))), "{}", b.card(one)["log"]);
}

fn lane(b: &Board, t: i64) -> (String, String) {
    let l = &b.card(t)["lane"];
    (l["lane"].as_str().unwrap().to_string(), l["from"].as_str().unwrap().to_string())
}

#[test]
fn a_lane_on_the_task_or_its_goal_says_where_it_starts() {
    let b = new_board();
    let head = git_repo(&b);
    let g = b.side_by_side();
    let in_slot = b.task("In a slot", json!({"goal_id": g, "lane": "slot"}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.start_job(in_slot).st("cwd"), slot(&b, 1), "a slot without a goal base starts at the default branch");
    assert_eq!(git(Path::new(&slot(&b, 1)), &["rev-parse", "HEAD"]), head);
    assert!(b.start_job(in_slot).st("prompt").contains("Lane: slot (it's set on the task)"), "{}", b.start_job(in_slot).st("prompt"));

    b.post(&format!("goals/G{g}"), json!({"worktree_base": "origin/main"}));
    assert_eq!(b.get(&format!("goals/G{g}"))["lane"], "slot", "--worktrees puts the goal on the slot lane");
    let shared = b.task("Shared", json!({"goal_id": g, "lane": "shared"}));
    let auto = b.task("Auto", json!({"goal_id": g}));
    assert_eq!(lane(&b, auto), ("slot".into(), "goal".into()));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.start_job(shared).st("cwd"), b.repo(), "its own lane wins over the goal's");
    assert_eq!(b.start_job(auto).st("cwd"), slot(&b, 2));

    b.post(&format!("goals/G{g}"), json!({"lane": "shared"}));
    assert_eq!(b.get(&format!("goals/G{g}"))["lane"], "shared");
    b.post(&format!("goals/G{g}"), json!({"worktree_base": "off"}));
    assert_eq!(b.get(&format!("goals/G{g}"))["lane"], "shared", "turning worktrees off leaves a lane set by hand");
    let (code, msg) = b.post_err(&format!("goals/G{g}"), json!({"lane": "sideways"}));
    assert_eq!(code, 400);
    assert!(msg.contains("isn't a lane"), "{msg}");
}

#[test]
fn auto_takes_a_slot_when_wave_mates_overlap_list_no_files_or_have_the_shared_checkout() {
    let b = new_board();
    git_repo(&b);
    let g = b.side_by_side();
    let r = b.report(
        "tb.propose",
        "s1",
        json!({"goal": format!("G{g}"), "tasks": [
            "Apart A::x::1::::src/a.rs", "Apart B::x::1::::src/b.rs",
            "Over C::x::2::::src/c.rs", "Over D::x::2::::src/c.rs, src/d.rs",
            "Bare E::x::3", "Listed F::x::3::::src/f.rs",
            "Pinned G::x::4::lane=shared", "Pinned H::x::4::::src/h.rs::lane=slot",
            "Held I::x::5::::src/i.rs::lane=shared", "Apart J::x::5::::src/j.rs",
            "Apart K::x::6::::src/k.rs", "Started L::x::6::::src/l.rs",
        ]}),
    );
    let ids: Vec<i64> =
        r["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().trim_start_matches('T').parse().unwrap()).collect();
    // L started first and took the shared checkout, so K, though first in its wave, takes a slot.
    worklane::pin(&b.app, &board::get_task(&b.app, ids[11]).unwrap(), "shared").unwrap();
    let picks: Vec<(String, String)> = ids.iter().map(|t| lane(&b, *t)).collect();
    let auto = |l: &str| (l.to_string(), "auto".to_string());
    let own = |l: &str| (l.to_string(), "task".to_string());
    assert_eq!(
        picks,
        [
            auto("shared"), auto("slot"), auto("slot"), auto("slot"), auto("slot"), auto("slot"), own("shared"), own("slot"),
            own("shared"), auto("slot"), auto("slot"), auto("shared"),
        ],
        "one task of a wave has the shared checkout: each makes its own branch"
    );
    let warnings = r["warnings"].to_string();
    assert!(warnings.contains(&format!("Wave 3 runs tasks side by side, and T{} plans no files", ids[4])), "{warnings}");
    assert!(!warnings.contains("Wave 4") && !warnings.contains("Wave 1"), "{warnings}");
}

#[test]
fn auto_stays_in_the_shared_checkout_outside_a_wave_or_a_git_checkout() {
    let b = new_board();
    let g = b.side_by_side();
    let r = b.report("tb.propose", "s1", json!({"goal": format!("G{g}"), "tasks": ["One::x::1", "Two::x::1"]}));
    let t: i64 = r["created"][0].as_str().unwrap()[1..].parse().unwrap();
    assert_eq!(lane(&b, t), ("shared".into(), "auto".into()), "no git checkout: no pool to borrow from");
    let loose = b.task("Loose", json!({"goal_id": g}));
    assert_eq!(lane(&b, loose), ("shared".into(), "auto".into()));
}

#[test]
fn alone_is_the_alone_lane() {
    let b = new_board();
    let g = b.side_by_side();
    let t = b.task("Alone", json!({"goal_id": g, "alone": "goal"}));
    assert_eq!(b.card(t)["lane"]["set"], "alone");
    b.post(&format!("tasks/T{t}"), json!({"lane": "shared"}));
    assert!(b.card(t)["alone"].is_null(), "another lane takes it off running alone");
    b.post(&format!("tasks/T{t}"), json!({"lane": "alone"}));
    assert_eq!(b.card(t)["alone"], "goal");
    b.post(&format!("tasks/T{t}"), json!({"alone": "none"}));
    assert_eq!(b.card(t)["lane"]["set"], "auto");
    let (code, _) = b.post_err(&format!("tasks/T{t}"), json!({"lane": "slot", "alone": "goal"}));
    assert_eq!(code, 400);

    b.post(&format!("goals/G{g}"), json!({"lane": "alone"}));
    let (one, two) = (b.task("One", json!({"goal_id": g})), b.task("Two", json!({"goal_id": g})));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), [t], "on the goal's alone lane, one runs at a time");
    assert!(b.start_job(t).st("prompt").contains("runs alone in its goal"), "{}", b.start_job(t).st("prompt"));
    assert!(![one, two].iter().any(|x| b.started().contains(x)));
}

#[test]
fn an_older_board_puts_worktree_goals_and_alone_tasks_on_their_lanes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("board.db");
    let db = taskboardd::db::Db::open(&path).unwrap();
    db.x("INSERT INTO goals(name, worktree_base) VALUES ('Old', 'origin/main'), ('Plain', NULL)", p![]).unwrap();
    db.x("INSERT INTO tasks(title, alone) VALUES ('Old', 'board'), ('Plain', NULL)", p![]).unwrap();
    drop(db);
    let db = taskboardd::db::Db::open(&path).unwrap();
    let lanes = |table: &str| -> Vec<Option<String>> {
        db.q(&format!("SELECT lane FROM {table} ORDER BY id"), p![]).unwrap().iter().map(|r| r.s("lane").map(str::to_string)).collect()
    };
    assert_eq!(lanes("goals"), [Some("slot".into()), None]);
    assert_eq!(lanes("tasks"), [Some("alone".into()), None]);
}

/// Notes `files` (from the project folder) in the task's hook edit log.
fn edited(b: &Board, t: i64, files: &[&str]) {
    let dir = b.app.cfg.data.join("edits");
    std::fs::create_dir_all(&dir).unwrap();
    let lines: String = files.iter().map(|f| format!("s{t}\t{}/{f}\n", b.repo())).collect();
    std::fs::write(dir.join(format!("T{t}")), lines).unwrap();
}

fn working(b: &Board, t: i64) {
    b.app.db.x("UPDATE tasks SET status = 'working' WHERE id = ?", p![t]).unwrap();
}

#[test]
fn a_task_outside_a_goal_claims_a_slot_and_lands_on_the_slot_lane() {
    let b = new_board();
    let head = git_repo(&b);
    let t = b.task("Loose", json!({}));
    let out = b.post(&format!("tasks/T{t}/slot/claim"), json!({}));
    assert_eq!((out["slot"].clone(), out["path"].clone()), (json!("slot-1"), json!(slot(&b, 1))), "{out}");
    assert_eq!((out["brought"].clone(), out["left"].clone()), (json!([]), json!([])));
    assert_eq!(git(Path::new(&slot(&b, 1)), &["rev-parse", "HEAD"]), head);
    assert_eq!(lane(&b, t), ("slot".into(), "task".into()));
    assert!(b.card(t)["log"].to_string().contains("Works in a worktree slot: claimed slot-1"), "{}", b.card(t)["log"]);
    assert_eq!(b.post(&format!("tasks/T{t}/slot/claim"), json!({}))["slot"], "slot-1", "claiming again keeps it");
}

#[test]
fn a_task_on_the_alone_lane_takes_no_slot() {
    let b = new_board();
    git_repo(&b);
    let t = b.task("Alone", json!({"lane": "alone"}));
    let (code, msg) = b.post_err(&format!("tasks/T{t}/slot/claim"), json!({}));
    assert_eq!(code, 409);
    assert!(msg.contains("is on the alone lane") && msg.contains(&format!("tb task set T{t} --lane slot")), "{msg}");
    assert!(!Path::new(&slot(&b, 1)).exists());
}

#[test]
fn a_full_pool_leaves_the_lane_as_it_was() {
    let b = new_board();
    git_repo(&b);
    b.post("projects/webapp", json!({"min_slots": 1, "max_slots": 1}));
    let (one, two) = (b.task("One", json!({})), b.task("Two", json!({})));
    b.post(&format!("tasks/T{one}/slot/claim"), json!({}));
    let before = lane(&b, two);
    let (code, _) = b.post_err(&format!("tasks/T{two}/slot/claim"), json!({}));
    assert_eq!(code, 409);
    assert_eq!(lane(&b, two), before);
    assert!(!b.card(two)["log"].to_string().contains("Works in a worktree slot"));
}

#[test]
fn bring_moves_a_tracked_and_a_new_file_and_cleans_the_shared_copies() {
    let b = new_board();
    git_repo(&b);
    let repo = b.repo();
    let t = b.task("Loose", json!({}));
    std::fs::write(Path::new(&repo).join("README"), "webapp, changed\n").unwrap();
    std::fs::create_dir_all(Path::new(&repo).join("src")).unwrap();
    std::fs::write(Path::new(&repo).join("src/new.rs"), "fn main() {}\n").unwrap();
    std::fs::write(Path::new(&repo).join("other"), "not mine\n").unwrap();
    edited(&b, t, &["README", "src/new.rs"]);

    let out = b.post(&format!("tasks/T{t}/slot/claim"), json!({"bring": []}));
    assert_eq!(out["brought"], json!(["README", "src/new.rs"]), "{out}");
    assert_eq!(out["left"], json!([]));
    let s = Path::new(&slot(&b, 1)).to_path_buf();
    assert_eq!(std::fs::read_to_string(s.join("README")).unwrap(), "webapp, changed\n");
    assert_eq!(std::fs::read_to_string(s.join("src/new.rs")).unwrap(), "fn main() {}\n");
    assert!(!s.join("other").exists(), "only its own files go");
    assert_eq!(git(Path::new(&repo), &["status", "--porcelain", "--", "README", "src", "other"]), "?? other", "the shared copies are gone, others' are left");
    assert!(Path::new(out["patch"].as_str().unwrap()).exists(), "{out}");
    assert_eq!(lane(&b, t), ("slot".into(), "task".into()));
    assert!(b.card(t)["log"].to_string().contains("bringing"), "{}", b.card(t)["log"]);
}

#[test]
fn bring_refuses_a_file_another_task_has_edits_in() {
    let b = new_board();
    git_repo(&b);
    let repo = b.repo();
    let (t, other) = (b.task("Mine", json!({})), b.task("Theirs", json!({})));
    working(&b, other);
    std::fs::write(Path::new(&repo).join("README"), "both\n").unwrap();
    edited(&b, t, &["README"]);
    edited(&b, other, &["README"]);
    // Only this task's patches are logged; the other's log is from before patches were kept.
    patched(&b, t, "README", json!([{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "lines": ["-webapp", "+both"]}]));
    let before = lane(&b, t);

    let (code, msg) = b.post_err(&format!("tasks/T{t}/slot/claim"), json!({"bring": ["README"]}));
    assert_eq!(code, 409);
    assert!(msg.contains(&format!("can't bring README (T{other} has uncommitted edits in it too)")), "{msg}");
    assert!(!Path::new(&slot(&b, 1)).exists(), "no slot was taken");
    assert_eq!(std::fs::read_to_string(Path::new(&repo).join("README")).unwrap(), "both\n");
    assert_eq!(lane(&b, t), before);
}

/// Notes one edit's patch (Claude's `structuredPatch` hunks) to `file` in the task's hook patch log.
fn patched(b: &Board, t: i64, file: &str, patch: Value) {
    use std::io::Write;
    let dir = b.app.cfg.data.join("hunks");
    std::fs::create_dir_all(&dir).unwrap();
    let line = json!({"file": format!("{}/{file}", b.repo()), "patch": patch});
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("T{t}"))).unwrap();
    writeln!(f, "{line}").unwrap();
}

/// A twelve-line `list` on main, here and on origin, and two tasks with edits in it: `mine` changed
/// line 2 and `theirs` (working) line 10, each through Claude's Edit.
fn shared_list(b: &Board) -> (i64, i64) {
    git_repo(b);
    let repo = Path::new(&b.repo()).to_path_buf();
    let lines: String = (1..=12).map(|n| format!("{n}\n")).collect();
    std::fs::write(repo.join("list"), lines).unwrap();
    git(&repo, &["add", "list"]);
    git(&repo, &["commit", "-q", "-m", "List"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    let (mine, theirs) = (b.task("Mine", json!({})), b.task("Theirs", json!({})));
    working(b, theirs);
    write_list(b, &[(2, "two (mine)"), (10, "ten (theirs)")]);
    edited(b, mine, &["list"]);
    edited(b, theirs, &["list"]);
    patched(b, mine, "list", json!([{"oldStart": 1, "oldLines": 5, "newStart": 1, "newLines": 5, "lines": [" 1", "-2", "+two (mine)", " 3", " 4", " 5"]}]));
    patched(b, theirs, "list", json!([{"oldStart": 7, "oldLines": 6, "newStart": 7, "newLines": 6, "lines": [" 7", " 8", " 9", "-10", "+ten (theirs)", " 11", " 12"]}]));
    (mine, theirs)
}

/// The shared `list` with these lines changed.
fn write_list(b: &Board, changed: &[(usize, &str)]) {
    let text: String = (1..=12).map(|n| format!("{}\n", changed.iter().find(|(m, _)| *m == n).map_or(n.to_string(), |(_, l)| l.to_string()))).collect();
    std::fs::write(Path::new(&b.repo()).join("list"), text).unwrap();
}

fn list_in(dir: &str) -> String {
    std::fs::read_to_string(Path::new(dir).join("list")).unwrap()
}

/// `list`'s twelve lines with these changed, as `write_list` writes them.
fn list_with(changed: &[(usize, &str)]) -> String {
    (1..=12).map(|n| format!("{}\n", changed.iter().find(|(m, _)| *m == n).map_or(n.to_string(), |(_, l)| l.to_string()))).collect()
}

#[test]
fn two_tasks_in_one_file_each_keep_their_own_hunks() {
    let b = new_board();
    let (mine, _) = shared_list(&b);
    let repo = b.repo();

    let out = b.post(&format!("tasks/T{mine}/slot/claim"), json!({"bring": []}));
    assert_eq!((out["brought"].clone(), out["split"].clone()), (json!(["list"]), json!(["list"])), "{out}");
    assert_eq!(list_in(&slot(&b, 1)), list_with(&[(2, "two (mine)")]), "the slot gets its own hunk only");
    assert_eq!(list_in(&repo), list_with(&[(10, "ten (theirs)")]), "the other task's hunk stays, its own is gone");
    assert_eq!(git(Path::new(&repo), &["diff", "--cached", "--name-only"]), "", "nothing of its is staged in the shared checkout");
    assert_eq!(lane(&b, mine), ("slot".into(), "task".into()));
}

#[test]
fn a_hunk_two_tasks_changed_is_refused_naming_the_other() {
    let b = new_board();
    let (mine, theirs) = shared_list(&b);
    let repo = b.repo();
    // The other task also changed line 3, next to this task's line 2: git sees one hunk.
    write_list(&b, &[(2, "two (mine)"), (3, "three (theirs)"), (10, "ten (theirs)")]);
    patched(&b, theirs, "list", json!([{"oldStart": 1, "oldLines": 6, "newStart": 1, "newLines": 6, "lines": [" 1", " two (mine)", "-3", "+three (theirs)", " 4", " 5", " 6"]}]));
    let before = lane(&b, mine);

    let (code, msg) = b.post_err(&format!("tasks/T{mine}/slot/claim"), json!({"bring": []}));
    assert_eq!(code, 409);
    assert!(msg.contains(&format!("can't bring list (T{theirs} changed the same lines as T{mine}, at line 2)")), "{msg}");
    assert!(!Path::new(&slot(&b, 1)).exists(), "no slot was taken");
    assert_eq!(list_in(&repo), list_with(&[(2, "two (mine)"), (3, "three (theirs)"), (10, "ten (theirs)")]));
    assert_eq!(lane(&b, mine), before);
}

#[test]
fn a_hunk_no_log_has_is_listed_then_brought_by_its_id() {
    let b = new_board();
    let (mine, _) = shared_list(&b);
    let repo = b.repo();
    // A shell edit: no task's log has it.
    write_list(&b, &[(2, "two (mine)"), (6, "six (shell)"), (10, "ten (theirs)")]);

    let (code, msg) = b.post_err(&format!("tasks/T{mine}/slot/claim"), json!({"bring": []}));
    assert_eq!(code, 409);
    assert!(msg.contains("(line 6: +six (shell))") && msg.contains("--bring-hunk none"), "{msg}");
    assert!(!Path::new(&slot(&b, 1)).exists(), "no slot was taken");
    let id = msg.split("list:").nth(1).unwrap().split(' ').next().unwrap().to_string();
    assert_eq!(id.len(), 6, "{msg}");

    let (code, msg) = b.post_err(&format!("tasks/T{mine}/slot/claim"), json!({"bring_hunk": [format!("{repo}/list:abcdef")]}));
    assert_eq!(code, 409);
    assert!(msg.contains("list:abcdef (no such hunk; its hunks are") && msg.contains(&id), "{msg}");

    let out = b.post(&format!("tasks/T{mine}/slot/claim"), json!({"bring_hunk": [format!("{repo}/list:{id}")]}));
    assert_eq!(out["brought"], json!(["list"]), "{out}");
    assert_eq!(list_in(&slot(&b, 1)), list_with(&[(2, "two (mine)"), (6, "six (shell)")]));
    assert_eq!(list_in(&repo), list_with(&[(10, "ten (theirs)")]));
}

#[test]
fn leaving_hunks_no_log_has_brings_its_own_and_lists_the_rest() {
    let b = new_board();
    let (mine, _) = shared_list(&b);
    let repo = b.repo();
    write_list(&b, &[(2, "two (mine)"), (6, "six (shell)"), (10, "ten (theirs)")]);

    let out = b.post(&format!("tasks/T{mine}/slot/claim"), json!({"bring_hunk": ["none"]}));
    assert!(out["left_hunks"][0].as_str().unwrap().contains("(line 6: +six (shell))"), "{out}");
    assert_eq!(list_in(&slot(&b, 1)), list_with(&[(2, "two (mine)")]));
    assert_eq!(list_in(&repo), list_with(&[(6, "six (shell)"), (10, "ten (theirs)")]));
}

#[test]
fn a_shared_copy_that_cant_be_written_leaves_everything_as_it_was() {
    use std::os::unix::fs::PermissionsExt;
    let b = new_board();
    let (mine, _) = shared_list(&b);
    let repo = b.repo();
    let file = Path::new(&repo).join("list");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    let before = lane(&b, mine);

    let (code, msg) = b.post_err(&format!("tasks/T{mine}/slot/claim"), json!({"bring": []}));
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(code, 409);
    assert!(msg.contains("couldn't take its hunks out of list") && msg.contains("the slot was given back"), "{msg}");
    assert_eq!(list_in(&repo), list_with(&[(2, "two (mine)"), (10, "ten (theirs)")]));
    assert_eq!(git(Path::new(&slot(&b, 1)), &["status", "--porcelain"]), "", "the slot is as it was made");
    assert!(slot_row(&b, 1).i("task_id").is_none(), "the slot is free again");
    assert_eq!(lane(&b, mine), before);
}

#[test]
fn a_clash_leaves_both_sides_untouched() {
    let b = new_board();
    git_repo(&b);
    let repo = b.repo();
    std::fs::write(Path::new(&repo).join("README"), "local\n").unwrap();
    git(Path::new(&repo), &["commit", "-qam", "Local only"]);
    std::fs::write(Path::new(&repo).join("README"), "local, edited\n").unwrap();
    let t = b.task("Loose", json!({}));
    let before = lane(&b, t);

    let (code, msg) = b.post_err(&format!("tasks/T{t}/slot/claim"), json!({"bring": ["README"]}));
    assert_eq!(code, 409);
    assert!(msg.contains("couldn't bring README into slot-1") && msg.contains("the slot was given back"), "{msg}");
    let patch = msg.rsplit("The patch is saved at ").next().unwrap().trim_end_matches('.');
    assert!(Path::new(patch).exists(), "{msg}");
    assert_eq!(std::fs::read_to_string(Path::new(&repo).join("README")).unwrap(), "local, edited\n");
    assert_eq!(git(Path::new(&slot(&b, 1)), &["status", "--porcelain"]), "", "the slot is as it was made");
    assert!(slot_row(&b, 1).i("task_id").is_none(), "the slot is free again");
    assert_eq!(lane(&b, t), before);
}
