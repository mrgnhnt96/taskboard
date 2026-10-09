//! What a device is and how to boot it (#92): kind (a label, never a tag), target, and start and stop
//! commands, set through the API, shown with the name, and told to the task that's lent it.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{handoff, p, runner};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn task(&self, title: &str, devices: &str) -> i64 {
        self.post("/tasks", json!({"title": title, "detail": "Do it.", "project": "webapp", "devices": devices}))["id"].as_i64().unwrap()
    }
    fn started(&self) -> Vec<i64> {
        self.app.db.q("SELECT DISTINCT task_id FROM jobs WHERE kind = 'agent' AND purpose = 'start' ORDER BY task_id", p![]).unwrap().iter().map(|j| j.i0("task_id")).collect()
    }
}

#[test]
fn a_device_says_what_it_is_and_how_to_start_and_stop_it() {
    let b = new_board();
    let d = b.post(
        "devices",
        json!({"name": "dev-a", "tags": "android", "kind": "android", "target": "Android 14",
               "start_cmd": "emulator -avd {device} -no-snapshot  # {task}", "stop_cmd": "adb -s {device} emu kill"}),
    );
    assert_eq!(d["label"], "dev-a (Android emulator, Android 14)");
    assert_eq!((d["kind"].clone(), d["kind_label"].clone(), d["target"].clone()), (json!("android"), json!("Android emulator"), json!("Android 14")));
    assert_eq!(d["start_cmd"], "emulator -avd {device} -no-snapshot  # {task}", "kept as given; filled for the task");

    // The kind is a label: a need for "phone" doesn't match it, its tag does.
    b.post("devices", json!({"name": "sim-b", "kind": "phone"}));
    let phone = b.task("Phone", "phone");
    let a = b.task("A", "android");
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![a], "kind phone isn't a tag: the phone task waits");
    assert!(b.get(&format!("tasks/T{phone}"))["devices"]["lent"].as_array().unwrap().is_empty());

    let card = b.get(&format!("tasks/T{a}"))["devices"].clone();
    assert_eq!(card["lent"], json!(["dev-a"]));
    assert_eq!(card["lent_labels"], json!(["dev-a (Android emulator, Android 14)"]));
    let h = handoff::build(&b.app, a).unwrap();
    assert!(h.contains("The board lent this task the device dev-a (Android emulator, Android 14): use only that one"), "{h}");
    assert!(h.contains(&format!("Start it: emulator -avd dev-a -no-snapshot  # T{a}")), "{h}");
    assert!(h.contains("Stop it when you're done: adb -s dev-a emu kill"), "{h}");

    // Set changes them; none clears.
    let d = b.post("devices/dev-a", json!({"target": "Android 15", "kind": "none", "stop_cmd": "none"}));
    assert_eq!(d["label"], "dev-a (Android 15)");
    assert!(d["kind"].is_null() && d["stop_cmd"].is_null());
    let h = handoff::build(&b.app, a).unwrap();
    assert!(h.contains("Start it:") && !h.contains("Stop it when"), "{h}");

    // Free-form kinds show as given.
    let d = b.post("devices/sim-b", json!({"kind": "Bench rig", "target": "rev C"}));
    assert_eq!(d["label"], "sim-b (Bench rig, rev C)");
    let pool = b.get("devices");
    assert_eq!(pool["devices"][0]["label"], "dev-a (Android 15)");
}

#[test]
fn several_lent_devices_each_get_their_own_start_and_stop_lines() {
    let b = new_board();
    b.post("devices", json!({"name": "pixel-7", "tags": "android", "start_cmd": "boot {device} for {title}"}));
    b.post("devices", json!({"name": "pixel-8", "tags": "android"}));
    let a = b.task("Two phones", "android:2");
    runner::start_queued(&b.app).unwrap();
    let h = handoff::build(&b.app, a).unwrap();
    assert!(h.contains("pixel-7:\n  Start it: boot pixel-7 for Two phones"), "{h}");
    assert!(!h.contains("pixel-8:"), "no commands, no lines: {h}");
}

#[test]
fn an_older_board_gains_the_device_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("board.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE devices(id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, tags TEXT DEFAULT '[]', focus TEXT, note TEXT,
               off INT DEFAULT 0, created_at TEXT, updated_at TEXT);
             INSERT INTO devices(name) VALUES ('old-one');",
        )
        .unwrap();
    let db = taskboardd::db::Db::open(&path).unwrap();
    let r = db.q1("SELECT kind, target, start_cmd, stop_cmd FROM devices WHERE name = 'old-one'", p![]).unwrap().unwrap();
    assert!(r.s("kind").is_none() && r.s("target").is_none() && r.s("start_cmd").is_none() && r.s("stop_cmd").is_none());
}

// --- #97 ---

#[test]
fn kinds_are_named_as_the_python_board_named_them() {
    let b = new_board();
    for (name, kind, label) in [
        ("d-1", "android", "Android emulator"),
        ("d-2", "ios", "iOS simulator"),
        ("d-3", "device", "Phone or tablet"),
        ("d-4", "other", "Device"),
        ("d-5", "Bench rig", "Bench rig"),
    ] {
        let d = b.post("devices", json!({"name": name, "kind": kind}));
        assert_eq!(d["kind_label"], label, "{kind}");
    }
    let d = b.post("devices", json!({"name": "plain", "target": "emulator-5556"}));
    assert_eq!(d["kind_label"], "Device", "no kind is a Device");
    assert_eq!(d["label"], "plain (emulator-5556)", "and isn't called one next to its name");
}

#[test]
fn a_long_handoff_keeps_the_device_lines_whole() {
    let b = new_board();
    b.post(
        "devices",
        json!({"name": "dev-c", "tags": "ios", "kind": "ios", "target": "ABCD-UDID",
               "start_cmd": "xcrun simctl boot {target} && open -a Simulator", "stop_cmd": "xcrun simctl shutdown {target}"}),
    );
    let long = "Step by step, with a great deal of detail. ".repeat(250);
    let a = b.post("/tasks", json!({"title": "Long one", "detail": long, "project": "webapp", "devices": "ios"}))["id"].as_i64().unwrap();
    runner::start_queued(&b.app).unwrap();
    let h = handoff::build(&b.app, a).unwrap();
    assert!(h.chars().count() <= 6000, "the handoff still fits: {}", h.chars().count());
    assert!(h.contains("The board lent this task the device dev-c (iOS simulator, ABCD-UDID): use only that one"), "{h}");
    assert!(h.contains("Start it: xcrun simctl boot ABCD-UDID && open -a Simulator"), "{h}");
    assert!(h.contains("Stop it when you're done: xcrun simctl shutdown ABCD-UDID"), "{h}");
    assert!(h.find("Start it:").unwrap() < h.find("What to do:").unwrap(), "the device lines come before the detail");
}

#[test]
fn device_commands_and_goal_setup_fill_the_task_and_device_placeholders() {
    let b = new_board();
    b.post("devices", json!({"name": "emu-1", "tags": "android", "target": "emulator-5554", "start_cmd": "boot {device} {target} for {n} in wave {wave} of {goal} ({jira})"}));
    b.post("devices", json!({"name": "emu-2", "tags": "android", "target": "emulator-5556"}));
    let g = b.post("/goals", json!({"name": "Phones", "project": "webapp"}))["id"].as_i64().unwrap();
    b.post(&format!("/goals/G{g}"), json!({"setup": "Use {device} ({target}) and {device2} ({target2}) for {task}."}));
    let a = b.post("/tasks", json!({"title": "Two", "detail": "Do it.", "project": "webapp", "goal_id": g, "wave": 2, "devices": "android:2"}))["id"].as_i64().unwrap();
    runner::start_queued(&b.app).unwrap();
    let lent = taskboardd::devices::lent(&b.app, a).unwrap();
    assert_eq!(lent.len(), 2, "both lent");
    let target = |n: &str| if n == "emu-1" { "emulator-5554" } else { "emulator-5556" };
    let h = handoff::build(&b.app, a).unwrap();
    assert!(h.contains(&format!("Start it: boot emu-1 emulator-5554 for {a} in wave 2 of G{g} (T{a})")), "{{jira}} falls back to the task's ref: {h}");
    assert!(h.contains(&format!("Use {} ({}) and {} ({}) for T{a}.", lent[0], target(&lent[0]), lent[1], target(&lent[1]))), "{h}");
    assert!(h.contains("They go back when the task is done."), "{h}");
}

#[test]
fn one_device_goes_back_and_the_others_in_use_are_named() {
    let b = new_board();
    b.post("devices", json!({"name": "dev-a", "tags": "android"}));
    b.post("devices", json!({"name": "dev-b", "tags": "android"}));
    let a = b.task("A", "android");
    let c = b.task("C", "android");
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![a, c]);
    let mine = taskboardd::devices::lent(&b.app, a).unwrap();
    let theirs = taskboardd::devices::lent(&b.app, c).unwrap();
    let h = handoff::build(&b.app, a).unwrap();
    assert!(h.contains("use only that one, since other tasks have the rest of the pool. It goes back when the task is done."), "{h}");
    assert!(h.contains(&format!("Other devices in use, don't touch them: {} (T{c}).", theirs[0])), "{h}");
    assert!(!h.contains(&format!("{} (T{a})", mine[0])), "its own device isn't listed as another's: {h}");

    // Nobody else has one: no such line.
    let solo = new_board();
    solo.post("devices", json!({"name": "dev-a", "tags": "android"}));
    let s = solo.task("Solo", "android");
    runner::start_queued(&solo.app).unwrap();
    assert!(!handoff::build(&solo.app, s).unwrap().contains("Other devices in use"));
}
