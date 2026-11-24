//! Stopping what a task's agent left running: Chrome on a debugging port, Gradle and Kotlin daemons, the
//! pool's booted simulators and what a gone terminal left behind, once the task is done with them, and
//! never what a working task, another task's build or the owner still uses. Simulators are leased: one no
//! task leased goes after a grace, and at once while more are booted than the pool has. A pool emulator no
//! task leased goes after a grace too; one outside the pool is never touched.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::leftovers::{pass, Act, Proc};
use taskboardd::p;
use taskboardd::util::{iso, now_ts, RowExt};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }

    /// A working task on terminal `s`, which it took `ago` seconds back.
    fn task(&self, title: &str, s: &str, ago: f64) -> i64 {
        let t = self.post("/tasks", json!({"title": title, "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        self.set(t, "working");
        self.terminal(t, s, ago);
        t
    }

    fn terminal(&self, t: i64, s: &str, ago: f64) {
        self.app.db.x("INSERT OR IGNORE INTO sessions(id, name) VALUES (?, ?)", p![s, s]).unwrap();
        self.app.db.x("INSERT INTO task_terminals(task_id, session_id, why, at) VALUES (?, ?, 'start', ?)", p![t, s, iso(now_ts() - ago)]).unwrap();
    }

    fn set(&self, t: i64, status: &str) {
        self.app.db.x("UPDATE tasks SET status = ? WHERE id = ?", p![status, t]).unwrap();
    }

    fn gone(&self, s: &str, ago: f64) {
        self.app.db.x("UPDATE sessions SET gone_at = ? WHERE id = ?", p![iso(now_ts() - ago), s]).unwrap();
    }

    /// Runs a pass over `procs` and returns what it did.
    fn pass(&self, procs: &[Proc]) -> Vec<Act> {
        let mut acts = vec![];
        pass(&self.app, procs, &mut |a| {
            acts.push(a.clone());
            true
        })
        .unwrap();
        acts
    }

    fn notes(&self, t: i64) -> Vec<String> {
        self.app
            .db
            .q("SELECT text FROM events WHERE task_id = ? AND kind = 'note' AND text LIKE 'Stopped%' ORDER BY id", p![t])
            .unwrap()
            .iter()
            .map(|r| r.st("text"))
            .collect()
    }
}

fn proc(pid: i64, ago: f64, session: Option<&str>, args: &[&str]) -> Proc {
    Proc {
        pid,
        ppid: 1,
        started: (now_ts() - ago) as i64,
        exe: args[0].to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        session: session.map(str::to_string),
        cwd: None,
    }
}

const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const JAVA: &str = "/Applications/Android Studio.app/Contents/jbr/Contents/Home/bin/java";

fn chrome(pid: i64, ago: f64, session: Option<&str>, port: u16, profile: &str) -> Proc {
    proc(pid, ago, session, &[CHROME, &format!("--remote-debugging-port={port}"), &format!("--user-data-dir={profile}")])
}

fn gradle(pid: i64, ago: f64, session: Option<&str>) -> Proc {
    proc(pid, ago, session, &[JAVA, "-cp", "gradle.jar", "org.gradle.launcher.daemon.bootstrap.GradleDaemon", "9.3.1"])
}

#[test]
fn a_done_tasks_chrome_is_stopped_and_its_profile_removed() {
    let b = new_board();
    let t = b.task("Check the page", "aa000001", 600.0);
    let profile = std::env::temp_dir().join(format!("ultrahand-drive-test-{}", std::process::id()));
    std::fs::create_dir_all(&profile).unwrap();
    let c = chrome(90001, 300.0, Some("aa000001"), 9742, &profile.to_string_lossy());

    assert_eq!(b.pass(&[c.clone()]), vec![], "the task is still working");
    b.set(t, "done");
    assert_eq!(b.pass(&[c.clone()]), vec![Act::Term(90001)]);
    assert_eq!(b.notes(t), vec!["Stopped what was left running: Chrome on port 9742"]);
    assert_eq!(b.pass(&[c]), vec![], "waits a moment before SIGKILL");

    assert_eq!(b.pass(&[]), vec![Act::Remove(profile.clone())], "gone: its temp profile goes too");
    let row = b.app.db.q1("SELECT * FROM leftovers WHERE pid = 90001", p![]).unwrap().unwrap();
    assert_eq!((row.i("task_id"), row.st("how")), (Some(t), "stopped".to_string()));
    assert!(row.s("cleaned_at").is_some());
    assert_eq!(b.pass(&[]), vec![]);
    std::fs::remove_dir_all(&profile).ok();
}

#[test]
fn a_process_belongs_to_the_task_its_terminal_was_on_when_it_started() {
    let b = new_board();
    let first = b.task("First", "aa000002", 3600.0);
    b.set(first, "done");
    let second = b.task("Second", "aa000002", 600.0);
    let early = chrome(90011, 1800.0, Some("aa000002"), 9001, "/nowhere/a");
    let late = chrome(90012, 300.0, Some("aa000002"), 9002, "/nowhere/b");

    assert_eq!(b.pass(&[early.clone(), late.clone()]), vec![Act::Term(90011)], "the first task's Chrome only");
    assert_eq!(b.notes(first), vec!["Stopped what was left running: Chrome on port 9001"]);
    assert_eq!(b.notes(second), Vec::<String>::new());
}

#[test]
fn a_gone_terminal_leaves_nothing_behind_but_the_owners_apps() {
    let b = new_board();
    let t = b.task("Carried on elsewhere", "aa000003", 7200.0);
    b.terminal(t, "aa000004", 60.0);
    let procs = [
        proc(90021, 3000.0, Some("aa000003"), &["node", "vite", "--port", "5173"]),
        proc(90022, 3000.0, Some("aa000003"), &["/Applications/Zen.app/Contents/MacOS/zen"]),
        proc(90023, 3000.0, Some("aa000003"), &["sccache"]),
        proc(90024, 3000.0, Some("aa000004"), &["node", "server.js"]),
    ];

    assert_eq!(b.pass(&procs), vec![], "the terminal is still there");
    b.gone("aa000003", 60.0);
    assert_eq!(b.pass(&procs), vec![], "just gone: give it a moment");
    b.gone("aa000003", 600.0);
    assert_eq!(b.pass(&procs), vec![Act::Term(90021)], "the dev server only; the task's new terminal is untouched");
    assert_eq!(b.notes(t), vec!["Stopped what was left running: node (pid 90021)"]);
}

#[test]
fn a_gone_terminal_with_something_still_under_midna_is_left_alone() {
    let b = new_board();
    b.task("Still attached", "aa000005", 7200.0);
    b.gone("aa000005", 600.0);
    let mut shell = proc(90031, 3000.0, Some("aa000005"), &["-zsh"]);
    shell.ppid = 777;
    let server = proc(90032, 3000.0, Some("aa000005"), &["node", "server.js"]);
    assert_eq!(b.pass(&[shell, server]), vec![]);
}

#[test]
fn gradle_daemons_wait_for_running_builds_and_the_owners_own_are_never_touched() {
    let b = new_board();
    let t = b.task("Android build", "aa000006", 7200.0);
    b.set(t, "done");
    let ours = gradle(90041, 3000.0, Some("aa000006"));
    let owners = gradle(90042, 90000.0, None);
    let build = proc(90043, 10.0, None, &[JAVA, "-cp", "gradle-wrapper.jar", "org.gradle.wrapper.GradleWrapperMain", "assemble"]);

    assert_eq!(b.pass(&[ours.clone(), owners.clone(), build]), vec![], "a build is running");
    assert_eq!(b.pass(&[ours, owners]), vec![Act::Term(90041)]);
    assert_eq!(b.notes(t), vec!["Stopped what was left running: a Gradle daemon"]);
}

#[test]
fn chrome_with_no_task_goes_once_its_older_than_orphan_hours() {
    let b = new_board();
    let young = chrome(90051, 3600.0, None, 9100, "/nowhere/c");
    let old = chrome(90052, 13.0 * 3600.0, None, 9101, "/nowhere/d");
    let owners = proc(90053, 90000.0, None, &[CHROME]);
    assert_eq!(b.pass(&[young, old, owners]), vec![Act::Term(90052)]);
}

#[test]
fn a_fresh_chrome_belongs_to_the_task_holding_its_slot() {
    let b = new_board();
    let t = b.task("Drive the UI", "aa000007", 600.0);
    let slot = b.dir.path().join("webapp/.claude/worktrees/slot-1");
    b.app
        .db
        .x("INSERT INTO worktree_slots(repo, n, path, task_id) VALUES ('webapp', 1, ?, ?)", p![slot.to_string_lossy(), t])
        .unwrap();
    let mut c = chrome(90061, 30.0, None, 9200, "/nowhere/e");
    c.cwd = Some(slot.join("app").to_string_lossy().into_owned());

    assert_eq!(b.pass(&[c.clone()]), vec![]);
    b.set(t, "done");
    assert_eq!(b.pass(&[c]), vec![Act::Term(90061)], "stopped with the task, not after orphan_hours");
    assert_eq!(b.notes(t), vec!["Stopped what was left running: Chrome on port 9200"]);
}

const LAUNCHD_SIM: &str = "/Library/Developer/PrivateFrameworks/CoreSimulator.framework/Resources/bin/launchd_sim";

/// A booted simulator, as `launchd_sim` shows it.
fn sim(pid: i64, ago: f64, udid: &str) -> Proc {
    proc(pid, ago, None, &[LAUNCHD_SIM, &format!("/Users/m/Library/Developer/CoreSimulator/Devices/{}/data/var/run/launchd_bootstrap.plist", udid.to_lowercase())])
}

const SIM_A: &str = "10C58C88-F29D-4B4B-9160-338B32728665";
const SIM_B: &str = "82CEBEAA-1C06-47C1-9E2E-481C3C268E94";
const OUTSIDE: &str = "0A1B2C3D-0000-4000-8000-000000000001";
const OUTSIDE_2: &str = "0A1B2C3D-0000-4000-8000-000000000002";

impl Board {
    /// A pool simulator whose stop command is `xcrun simctl shutdown {target}`.
    fn pool_sim(&self, name: &str, udid: &str) {
        self.post("devices", json!({"name": name, "kind": "iphone", "tags": "ios", "target": udid, "stop_cmd": "xcrun simctl shutdown {target}"}));
    }

    fn lend(&self, device: &str, t: i64) {
        self.app.db.x("INSERT INTO device_loans(device, task_id, at) VALUES (?, ?, ?)", p![device, t, iso(now_ts() - 500.0)]).unwrap();
    }
}

#[test]
fn a_pool_simulator_is_shut_down_once_its_loan_ends() {
    let b = new_board();
    b.pool_sim("kaepora-iphone-2", SIM_A);
    let t = b.task("Phone check", "aa000008", 600.0);
    b.lend("kaepora-iphone-2", t);
    let s = sim(90071, 4000.0, SIM_A);

    assert_eq!(b.pass(&[s.clone()]), vec![], "lent to a working task");
    b.app.db.x("UPDATE device_loans SET released_at = ?", p![iso(now_ts())]).unwrap();
    b.set(t, "done");
    assert_eq!(b.pass(&[s.clone()]), vec![Act::Stop(format!("xcrun simctl shutdown {SIM_A}"))]);
    assert_eq!(b.notes(t), vec!["Stopped what was left running: kaepora-iphone-2"]);
    assert_eq!(b.pass(&[s]), vec![], "no SIGKILL for a simulator");
}

#[test]
fn a_simulator_no_task_leased_is_shut_down_after_the_grace() {
    let b = new_board();
    b.pool_sim("kaepora-iphone-2", SIM_A);
    b.pool_sim("kaepora-iphone-3", SIM_B);
    let young = [sim(90081, 60.0, SIM_A), sim(90082, 60.0, OUTSIDE)];
    assert_eq!(b.pass(&young), vec![], "booted a minute ago, and no more than the pool has");

    let grace = b.app.cfg.cleanup.unleased_simulator_mins * 60.0;
    let old = [sim(90083, grace + 60.0, SIM_B), sim(90084, grace + 60.0, OUTSIDE_2)];
    let mut acts = b.pass(&old);
    acts.sort_by_key(|a| format!("{a:?}"));
    assert_eq!(
        acts,
        vec![Act::Stop(format!("xcrun simctl shutdown {}", OUTSIDE_2)), Act::Stop(format!("xcrun simctl shutdown {SIM_B}"))],
        "in the pool or not, a simulator nobody leased goes"
    );
    let what: Vec<String> = b.app.db.q("SELECT what FROM leftovers WHERE pid = 90084", p![]).unwrap().iter().map(|r| r.st("what")).collect();
    assert_eq!(what, vec![format!("simulator {}", OUTSIDE_2)]);
}

#[test]
fn no_more_simulators_stay_up_than_the_pool_has() {
    let b = new_board();
    b.pool_sim("kaepora-iphone-2", SIM_A);
    let t = b.task("Phone check", "aa000009", 600.0);
    b.lend("kaepora-iphone-2", t);
    let lent = sim(90091, 300.0, SIM_A);
    let extra = sim(90092, 30.0, OUTSIDE);

    assert_eq!(b.pass(&[lent.clone(), extra.clone()]), vec![Act::Stop(format!("xcrun simctl shutdown {OUTSIDE}"))], "over the pool's one: the unleased one goes at once");
    assert_eq!(b.pass(&[lent.clone(), extra]), vec![], "asked once; the lent one stays");
    assert_eq!(b.pass(&[lent]), vec![]);
    assert_eq!(b.notes(t), Vec::<String>::new(), "nothing of the task's was stopped");
}

#[test]
fn an_unleased_simulator_waits_as_long_as_the_owner_set() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.cleanup.unleased_simulator_mins = 120.0;
    let b = Board { app: App::for_tests(cfg), dir };
    b.pool_sim("kaepora-iphone-2", SIM_A);
    assert_eq!(b.pass(&[sim(90101, 3600.0, SIM_A)]), vec![]);
    assert_eq!(b.pass(&[sim(90102, 3.0 * 3600.0, SIM_A)]), vec![Act::Stop(format!("xcrun simctl shutdown {SIM_A}"))]);
}

const QEMU: &str = "/Users/m/Library/Android/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64";

/// A booted emulator on `port`, as `qemu-system` shows it.
fn emu(pid: i64, ago: f64, port: u16) -> Proc {
    proc(pid, ago, None, &[QEMU, "-avd", "Pixel_8", "-port", &port.to_string()])
}

impl Board {
    /// A pool emulator with no stop command of its own (`adb -s {target} emu kill`).
    fn pool_emu(&self, name: &str, port: u16) {
        self.post("devices", json!({"name": name, "kind": "android", "tags": "android", "target": format!("emulator-{port}")}));
    }
}

#[test]
fn a_pool_emulator_no_task_leased_is_shut_down_after_the_grace() {
    let b = new_board();
    b.pool_emu("kaepora-pixel-1", 5570);
    b.pool_emu("kaepora-pixel-2", 5572);
    assert_eq!(b.pass(&[emu(90111, 60.0, 5570)]), vec![], "booted a minute ago");

    let grace = b.app.cfg.cleanup.unleased_emulator_mins * 60.0;
    assert!(grace < b.app.cfg.cleanup.orphan_hours * 3600.0);
    assert_eq!(b.pass(&[emu(90112, grace + 60.0, 5572)]), vec![Act::Stop("adb -s emulator-5572 emu kill".into())], "not left for orphan_hours");
    let what: Vec<String> = b.app.db.q("SELECT what FROM leftovers WHERE pid = 90112", p![]).unwrap().iter().map(|r| r.st("what")).collect();
    assert_eq!(what, vec!["kaepora-pixel-2"]);
}

#[test]
fn a_lent_emulator_stays_and_goes_once_its_loan_ends() {
    let b = new_board();
    b.pool_emu("kaepora-pixel-1", 5570);
    let t = b.task("Android check", "aa000011", 600.0);
    b.lend("kaepora-pixel-1", t);
    let e = emu(90121, 2.0 * 3600.0, 5570);

    assert_eq!(b.pass(&[e.clone()]), vec![], "lent to a working task, however long it's been up");
    b.app.db.x("UPDATE device_loans SET released_at = ?", p![iso(now_ts())]).unwrap();
    b.set(t, "done");
    assert_eq!(b.pass(&[e]), vec![Act::Stop("adb -s emulator-5570 emu kill".into())]);
    assert_eq!(b.notes(t), vec!["Stopped what was left running: kaepora-pixel-1"]);
}

#[test]
fn an_emulator_outside_the_pool_is_never_touched() {
    let b = new_board();
    b.pool_emu("kaepora-pixel-1", 5570);
    let old = b.app.cfg.cleanup.orphan_hours * 3600.0 + 60.0;
    assert_eq!(b.pass(&[emu(90131, old, 5580)]), vec![]);
    assert_eq!(b.pass(&[emu(90131, old, 5580)]), vec![]);
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM leftovers WHERE pid = 90131", p![]).unwrap(), 0, "not even recorded");
}

#[test]
fn an_unleased_emulator_waits_as_long_as_the_owner_set() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.cleanup.unleased_emulator_mins = 120.0;
    let b = Board { app: App::for_tests(cfg), dir };
    b.pool_emu("kaepora-pixel-1", 5570);
    assert_eq!(b.pass(&[emu(90141, 3600.0, 5570)]), vec![]);
    assert_eq!(b.pass(&[emu(90142, 3.0 * 3600.0, 5570)]), vec![Act::Stop("adb -s emulator-5570 emu kill".into())]);
}
