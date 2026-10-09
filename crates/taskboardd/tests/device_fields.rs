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
    assert_eq!(d["label"], "dev-a (Android phone, Android 14)");
    assert_eq!((d["kind"].clone(), d["kind_label"].clone(), d["target"].clone()), (json!("android"), json!("Android phone"), json!("Android 14")));
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
    assert_eq!(card["lent_labels"], json!(["dev-a (Android phone, Android 14)"]));
    let h = handoff::build(&b.app, a).unwrap();
    assert!(h.contains("The board lent this task the device dev-a (Android phone, Android 14): use only that one"), "{h}");
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
