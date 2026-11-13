//! What a device is and how to boot it (#92): kind (a label, never a tag), target, and start and stop
//! commands, set through the API, shown with the name, and told to the task that's lent it.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, handoff, p, runner};

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
    fn handoff(&self, t: i64) -> Value {
        let q: Query = [("task".to_string(), format!("T{t}"))].into_iter().collect();
        api::dispatch(&self.app, "GET", "/handoff", &q, &json!({})).unwrap_or_else(|e| panic!("GET /handoff: {}", e.message))["text"].clone()
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
    assert!(h.chars().count() <= handoff::CONTEXT_LIMIT, "the handoff still fits: {}", h.chars().count());
    assert!(h.contains("The board lent this task the device dev-c (iOS simulator, ABCD-UDID): use only that one"), "{h}");
    assert!(h.contains("Start it: xcrun simctl boot ABCD-UDID && open -a Simulator"), "{h}");
    assert!(h.contains("Stop it when you're done: xcrun simctl shutdown ABCD-UDID"), "{h}");
    assert!(h.find("Start it:").unwrap() < h.find("What to do:").unwrap(), "the device lines come before the detail");
    assert!(h.contains("[Shortened to fit. tb handoff prints it in full.]"), "the shortened detail says where the rest is: {h}");
    let full = b.handoff(a).as_str().unwrap().to_string();
    assert!(full.contains(long.trim()), "tb handoff prints the detail whole");
}

#[test]
fn a_long_handoff_points_to_its_reference_and_keeps_the_work_whole() {
    let b = new_board();
    b.post("devices", json!({"name": "dev-c", "tags": "ios", "kind": "ios", "target": "ABCD-UDID", "start_cmd": "xcrun simctl boot {target}"}));
    let g = b.post("/goals", json!({"name": "Tablet", "project": "webapp"}))["id"].as_i64().unwrap();
    let setup = format!("Boot {{device}} first. {}", "Sign in as the test user and open the library. ".repeat(25));
    b.post(&format!("/goals/G{g}"), json!({"setup": setup}));
    for n in 0..12 {
        b.post(
            &format!("/goals/G{g}/attachments"),
            json!({"url": format!("https://example.com/design/{n}"), "title": format!("Screen {n}"), "kind": "design",
                   "summary": "Every state of the screen, light and dark, with the spacing called out. ".repeat(6)}),
        );
    }
    let detail = format!("Fix the track list. {}", "The tester saw the rows jump when the list scrolls. ".repeat(20));
    let a = b.post("/tasks", json!({"title": "Track list", "detail": detail, "project": "webapp", "goal_id": g, "devices": "ios"}))["id"]
        .as_i64()
        .unwrap();
    runner::start_queued(&b.app).unwrap();
    let h = handoff::build(&b.app, a).unwrap();
    assert!(h.chars().count() <= handoff::CONTEXT_LIMIT, "it fits: {}", h.chars().count());
    assert!(h.contains(&format!("What to do:\n{}", detail.trim())), "the work is whole: {h}");
    assert!(h.contains(setup.replace("{device}", "dev-c").trim()), "the goal's setup is whole: {h}");
    assert!(h.contains("Attached (read these before you start): [Shortened to fit. tb handoff prints it in full.]"), "{h}");
    assert!(!h.contains("Screen 11"), "the attachments wait for tb handoff");
    let full = b.handoff(a).as_str().unwrap().to_string();
    assert!(full.contains("Screen 11") && !full.contains("Shortened to fit"), "tb handoff has every attachment: {full}");
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
    assert!(h.contains("use only that one. It goes back when the task is done."), "{h}");
    assert!(h.contains(&format!("Leave these devices alone, other tasks or goals have them: {}.", theirs[0])), "{h}");
    assert!(!h.contains(&format!("have them: {}", mine[0])), "its own device isn't listed as another's: {h}");

    // Nobody else has one: no such line.
    let solo = new_board();
    solo.post("devices", json!({"name": "dev-a", "tags": "android"}));
    let s = solo.task("Solo", "android");
    runner::start_queued(&solo.app).unwrap();
    assert!(!handoff::build(&solo.app, s).unwrap().contains("Leave these devices alone"));
}

// --- #105 ---

#[test]
fn goal_setup_fills_jira_with_the_ticket_or_the_task_ref() {
    let b = new_board();
    let g = b.post("/goals", json!({"name": "Branches", "project": "webapp"}))["id"].as_i64().unwrap();
    b.post(&format!("/goals/G{g}"), json!({"setup": "make-tree ../wt-{jira} -b feature/{jira}"}));
    let keyed = b.post("/tasks", json!({"title": "Keyed", "detail": "Do it.", "project": "webapp", "goal_id": g}))["id"].as_i64().unwrap();
    let bare = b.post("/tasks", json!({"title": "Bare", "detail": "Do it.", "project": "webapp", "goal_id": g}))["id"].as_i64().unwrap();
    board::update_task(&b.app, keyed, fields!["jira_key" => "PROJ-42"]).unwrap();
    let h = handoff::build(&b.app, keyed).unwrap();
    assert!(h.contains("make-tree ../wt-PROJ-42 -b feature/PROJ-42"), "{h}");
    let h = handoff::build(&b.app, bare).unwrap();
    assert!(h.contains(&format!("make-tree ../wt-T{bare} -b feature/T{bare}")), "no ticket: the task's ref: {h}");
    assert!(!h.contains("{jira}"), "{h}");
}

// --- #106 ---

#[test]
fn other_devices_are_named_only_and_a_long_list_points_to_tb_devices() {
    let b = new_board();
    b.post("devices", json!({"name": "dev-a", "tags": "android", "target": "emulator-5554"}));
    b.post("devices", json!({"name": "dev-b", "tags": "android", "target": "emulator-5556"}));
    b.post("devices", json!({"name": "res-1", "tags": "android", "target": "R1"}));
    b.post("devices", json!({"name": "off-1", "tags": "android"}));
    b.post("devices", json!({"name": "spare", "tags": "ios"}));
    b.post("devices/off-1", json!({"off": true, "note": "Kept for the demo"}));
    let other = b.post("/goals", json!({"name": "Other", "project": "webapp"}))["id"].as_i64().unwrap();
    b.post(&format!("goals/G{other}/devices"), json!({"device": "res-1", "reserved": true}));
    let a = b.task("A", "android");
    let c = b.task("C", "android");
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![a, c]);
    let theirs = taskboardd::devices::lent(&b.app, c).unwrap();
    let h = handoff::build(&b.app, a).unwrap();
    let mut want = vec![theirs[0].clone(), "off-1".to_string(), "res-1".to_string()];
    want.sort();
    assert!(h.contains(&format!("Leave these devices alone, other tasks or goals have them: {}.", want.join(", "))), "{h}");
    assert!(!h.contains("emulator-5556") && !h.contains("R1") && !h.contains("Kept for") && !h.contains("spare"), "names only: {h}");

    // A long list names a few and points to tb devices for the rest.
    for i in 0..30 {
        b.post("devices", json!({"name": format!("kept-{i:02}"), "tags": "tv"}));
        b.post(&format!("devices/kept-{i:02}"), json!({"off": true, "note": "Kept for a long demo booth story"}));
    }
    let h = handoff::build(&b.app, a).unwrap();
    let tb = board::tb_cmd(&b.app);
    let line = h.lines().find(|l| l.starts_with("Leave these devices alone")).unwrap();
    assert_eq!(
        line,
        format!("Leave these devices alone, other tasks or goals have them: {}, kept-00, kept-01, kept-02, kept-03 and 28 more ({tb} devices lists them).", theirs[0])
    );
    assert!(line.chars().count() < 200, "{line}");
}

#[test]
fn a_goal_setup_that_covers_the_lent_device_isnt_repeated() {
    let b = new_board();
    b.post(
        "devices",
        json!({"name": "ipad-sim", "tags": "ios", "kind": "ios", "target": "B3CF-UDID",
               "start_cmd": "xcrun simctl boot {target}", "stop_cmd": "xcrun simctl shutdown {target}"}),
    );
    b.app.db.x("UPDATE devices SET note = 'Open Simulator.app to watch it' WHERE name = 'ipad-sim'", p![]).unwrap();
    let g = b.post("/goals", json!({"name": "Tablet", "project": "webapp"}))["id"].as_i64().unwrap();
    b.post(
        &format!("/goals/G{g}"),
        json!({"setup": "Boot it: xcrun simctl boot {target}. Install with flutter install -d {target}, record with xcrun simctl io {target} recordVideo out.mov."}),
    );
    let a = b.post("/tasks", json!({"title": "Tab", "detail": "Do it.", "project": "webapp", "goal_id": g, "devices": "ios"}))["id"].as_i64().unwrap();
    runner::start_queued(&b.app).unwrap();
    let h = handoff::build(&b.app, a).unwrap();
    assert!(h.contains("The board lent this task the device ipad-sim (iOS simulator, B3CF-UDID): use only that one. It goes back when the task is done."), "{h}");
    assert!(!h.contains("Open Simulator.app"), "the goal's setup covers the device, so its note isn't repeated: {h}");
    assert!(!h.contains("Start it:"), "the setup already boots it: {h}");
    assert!(h.contains("Stop it when you're done: xcrun simctl shutdown B3CF-UDID"), "the setup doesn't stop it: {h}");
    assert_eq!(h.matches("xcrun simctl boot B3CF-UDID").count(), 1, "{h}");
}

#[test]
fn a_lent_devices_note_is_clipped_to_300() {
    let b = new_board();
    b.post("devices", json!({"name": "dev-a", "tags": "android"}));
    let long = "word ".repeat(120);
    b.app.db.x("UPDATE devices SET note = ? WHERE name = 'dev-a'", p![long.trim()]).unwrap();
    let a = b.task("A", "android");
    runner::start_queued(&b.app).unwrap();
    let h = handoff::build(&b.app, a).unwrap();
    let start = h.find("dev-a (android, ").unwrap() + "dev-a (android, ".len();
    let note: String = h[start..].chars().take_while(|c| *c != ')').collect();
    assert!(note.chars().count() <= 300 && note.ends_with('…'), "{}: {note}", note.chars().count());
}

// --- #113 ---

#[test]
fn a_lent_device_keeps_its_details_once_another_goal_reserves_it() {
    let b = new_board();
    b.post(
        "devices",
        json!({"name": "dev-c", "tags": "ios", "kind": "ios", "target": "ABCD-UDID",
               "start_cmd": "xcrun simctl boot {target}", "stop_cmd": "xcrun simctl shutdown {target}"}),
    );
    b.app.db.x("UPDATE devices SET note = 'The one on the left' WHERE name = 'dev-c'", p![]).unwrap();
    let a = b.task("A", "ios");
    runner::start_queued(&b.app).unwrap();
    assert_eq!(taskboardd::devices::lent(&b.app, a).unwrap(), vec!["dev-c".to_string()]);
    let other = b.post("/goals", json!({"name": "Other", "project": "webapp"}))["id"].as_i64().unwrap();
    b.post(&format!("goals/G{other}/devices"), json!({"device": "dev-c", "reserved": true}));
    let h = handoff::build(&b.app, a).unwrap();
    assert!(
        h.contains("The board lent this task the device dev-c (iOS simulator, ABCD-UDID, The one on the left): use only that one"),
        "{h}"
    );
    assert!(h.contains("Start it: xcrun simctl boot ABCD-UDID"), "{h}");
    assert!(h.contains("Stop it when you're done: xcrun simctl shutdown ABCD-UDID"), "{h}");
    assert!(!h.contains("Leave these devices alone"), "its own device isn't another's, reserved or not: {h}");
}

#[test]
fn a_task_with_no_device_lent_still_hears_which_to_leave_alone() {
    let b = new_board();
    b.post("devices", json!({"name": "dev-a", "tags": "android", "target": "emulator-5554"}));
    let a = b.task("A", "android");
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![a]);
    let others = "Leave these devices alone, other tasks or goals have them: dev-a.";
    // Queued behind A: it hasn't started, so the board lends it one later.
    let c = b.task("C", "android");
    assert!(taskboardd::devices::lent(&b.app, c).unwrap().is_empty());
    let h = handoff::build(&b.app, c).unwrap();
    assert!(h.contains("This task asks for devices (android). The board lends it to this task when it starts, and this handoff then says which."), "{h}");
    assert!(!h.contains("none were free") && h.contains(others), "{h}");

    // The handoff it starts with, and once it has started: none were free.
    let starting = handoff::build_starting(&b.app, c).unwrap();
    board::update_task(&b.app, c, fields!["started_at" => taskboardd::util::now_iso()]).unwrap();
    for h in [starting, handoff::build(&b.app, c).unwrap()] {
        assert!(h.contains("but none were free when it started."), "{h}");
        assert!(!h.contains("lends it to this task when it starts") && h.contains(others), "{h}");
    }
}

#[test]
fn a_task_with_no_device_needs_is_told_never_to_hunt_for_one_once_the_pool_has_any() {
    let b = new_board();
    let t = b.task("Copy", "");
    let tb = board::tb_cmd(&b.app);
    let line = format!(
        "Never look for, start or boot an emulator, simulator or phone yourself (no adb devices, xcrun simctl or emulator \
         commands): the board's devices stay shut down until it lends one. If this task needs one, run {tb} task set T{t} \
         --device tag:<tag> ({tb} devices lists the tags) and the board lends you a free one and says how to start it. \
         Otherwise don't use any."
    );
    assert!(!handoff::build(&b.app, t).unwrap().contains("Never look for, start or boot"), "an empty pool says nothing");
    b.post("devices", json!({"name": "pixel-a", "tags": "android"}));
    let h = handoff::build(&b.app, t).unwrap();
    assert!(h.contains(&line), "{h}");
    let a = b.task("A", "android");
    assert!(!handoff::build(&b.app, a).unwrap().contains("Never look for, start or boot"), "a task that asks for devices hears about those");
}
