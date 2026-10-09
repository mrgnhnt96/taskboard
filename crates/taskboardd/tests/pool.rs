//! The device pool (#27), bits (#34), and the owner's review stop and holding a wave (#31).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, fields, handoff, p, runner, waitsfor};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    f(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, _dir: dir }
}

fn new_board() -> Board {
    board_with(|_| {})
}

fn app_query() -> Query {
    let mut q = Query::new();
    q.insert(api::FROM.into(), "app".into());
    q
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
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
    fn started(&self) -> Vec<i64> {
        self.app.db.q("SELECT DISTINCT task_id FROM jobs WHERE kind = 'agent' AND purpose = 'start' ORDER BY task_id", p![]).unwrap().iter().map(|j| j.i0("task_id")).collect()
    }
    fn waiting(&self, id: i64) -> Value {
        waitsfor::waiting_line(&self.app, &board::get_task(&self.app, id).unwrap()).unwrap()
    }
    fn set(&self, id: i64, status: &str) {
        board::update_task(&self.app, id, fields!["status" => status, "start_job" => null, "finished_at" => taskboardd::util::now_iso()]).unwrap();
    }
    fn goal(&self) -> i64 {
        self.post("/goals", json!({"name": "Mobile", "project": "webapp", "run_in_order": false, "max_terminals": 4}))["id"].as_i64().unwrap()
    }
}

#[test]
fn a_task_waits_for_free_devices_is_lent_them_and_gives_them_back() {
    let b = new_board();
    b.post("devices", json!({"name": "pixel-7", "tags": "android, phone"}));
    b.post("devices", json!({"name": "Pixel-8", "tags": ["android"], "note": "Android 15"}));
    b.post("devices", json!({"name": "iphone-15", "tags": "ios"}));
    let (code, msg) = b.post_err("devices", json!({"name": "pixel-7"}));
    assert_eq!(code, 409, "{msg}");

    let a = b.task("A", json!({"devices": "android:2"}));
    let c = b.task("C", json!({"devices": ["android"]}));
    let i = b.task("I", json!({"devices": "ios"}));
    assert_eq!(b.get(&format!("tasks/T{a}"))["devices"]["needs_text"], "2 android");
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![a, i], "C waits: A has both android devices");
    let card = b.get(&format!("tasks/T{a}"));
    assert_eq!(card["devices"]["lent"], json!(["pixel-8", "pixel-7"]), "the narrowest devices go first");
    assert_eq!(b.waiting(c), format!("Waits for a android device (T{a} has them)"));
    let h = handoff::build(&b.app, a).unwrap();
    assert!(h.contains("The board lent this task the devices pixel-8 (android, Android 15), pixel-7 (android, phone)"), "{h}");

    let pool = b.get("devices");
    assert_eq!(pool["devices"][1]["held_by"]["ref"], format!("T{a}"));
    assert_eq!(pool["waiting"][0]["ref"], format!("T{c}"));
    let (code, _) = b.post_err("devices/pixel-7/remove", json!({}));
    assert_eq!(code, 409, "a lent device stays in the pool");

    b.set(a, "done");
    runner::tick(&b.app).unwrap();
    assert!(b.started().contains(&c), "A's devices came back, so C starts");
    assert_eq!(b.get(&format!("tasks/T{c}"))["devices"]["lent"], json!(["pixel-8"]));
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM device_loans WHERE task_id = ? AND released_at IS NULL", p![a]).unwrap(), 0);
}

#[test]
fn a_goal_asks_for_devices_for_its_tasks_and_the_pool_must_have_them() {
    let b = new_board();
    let g = b.goal();
    b.post(&format!("goals/G{g}"), json!({"devices": "ios"}));
    let t = b.task("Sim test", json!({"goal_id": g}));
    b.post(&format!("goals/G{g}/run"), json!({}));
    assert_eq!(b.waiting(t), "No ios yet (tb device add)");
    runner::start_queued(&b.app).unwrap();
    assert!(b.started().is_empty());
    assert!(board::is_held(&b.app, &board::get_task(&b.app, t).unwrap()).unwrap());
    assert_eq!(b.get(&format!("goals/G{g}"))["held"], 1);
    b.post("devices", json!({"name": "sim-a", "tags": "ios", "focus": "true"}));
    let d = b.get(&format!("goals/G{g}"));
    assert_eq!(d["devices"]["needs_text"], "ios");
    assert_eq!(d["devices"]["devices"][0]["can_focus"], true);
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![t]);
    b.post(&format!("tasks/T{t}"), json!({"devices": "none"}));
    b.post("devices/sim-a/focus", json!({}));
    let (code, msg) = b.post_err("devices/nope/focus", json!({}));
    assert_eq!(code, 404, "{msg}");
}

#[test]
fn a_device_without_a_focus_command_uses_the_configured_one() {
    let b = board_with(|c| c.devices.focus = "open -a Simulator --args {name}".into());
    b.post("devices", json!({"name": "sim-b", "tags": "ios"}));
    let d = b.app.db.q1("SELECT * FROM devices WHERE name = 'sim-b'", p![]).unwrap().unwrap();
    assert_eq!(taskboardd::devices::focus_command(&b.app, &d).as_deref(), Some("open -a Simulator --args sim-b"));
    let nb = new_board();
    nb.post("devices", json!({"name": "sim-c"}));
    let (code, msg) = nb.post_err("devices/sim-c/focus", json!({}));
    assert_eq!(code, 409);
    assert!(msg.contains("no focus command"), "{msg}");
}

#[test]
fn a_task_starts_behind_unmade_bits_and_a_goal_waits_on_them_after_its_tasks() {
    let b = board_with(|c| {
        c.bits.tool = "Flagsmith".into();
        c.bits.create_url = "https://flags.example.com/new?key={name}&p={project}".into();
    });
    let g = b.goal();
    let t = b.task("Checkout", json!({"goal_id": g}));
    let bit = b.post("bits", json!({"name": "newCheckout", "kind": "backend", "tasks": [format!("T{t}")], "who": "terminal ab"}));
    assert_eq!(bit["create_url"], "https://flags.example.com/new?key=newCheckout&p=webapp");
    assert_eq!(bit["waiting"], true);
    b.post("bits", json!({"name": "beta-banner", "kind": "local", "goals": [format!("G{g}")]}));
    let (code, msg) = b.post_err(&format!("tasks/T{t}"), json!({"bits": ["missing"]}));
    assert_eq!(code, 404, "{msg}");
    b.post(&format!("tasks/T{t}"), json!({"bits": "beta-banner"}));
    assert_eq!(b.get(&format!("tasks/T{t}"))["bits"].as_array().unwrap().len(), 2);

    let (code, msg) = b.post_err("bits/beta-banner/made", json!({}));
    assert_eq!(code, 409, "a local bit isn't made anywhere: {msg}");

    b.post(&format!("goals/G{g}/run"), json!({}));
    assert!(b.waiting(t).is_null(), "an unmade bit doesn't hold a task back: {}", b.waiting(t));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![t], "the task starts and builds behind the flag");
    let d = b.get(&format!("goals/G{g}"));
    assert_eq!(d["bits"]["backend"], 1);
    assert_eq!(d["bits"]["made"], 0);
    assert_eq!(d["held"], 0);
    assert_eq!(d["bits"]["list"].as_array().unwrap().len(), 2);
    let h = handoff::build(&b.app, t).unwrap();
    assert!(h.contains("beta-banner (local: in the code only, not in Flagsmith), newCheckout (backend: not made in Flagsmith yet)"), "{h}");
    assert!(h.contains("doesn't hold up your work: build behind the flag anyway"), "{h}");

    b.set(t, "done");
    let d = b.get(&format!("goals/G{g}"));
    assert_eq!(d["bits_waiting"], 1);
    assert!(d["finished_at"].is_null(), "an unmade backend bit holds back a finished goal");
    assert_eq!(d["state"], "Waiting on 1 bit");

    let made = b.post("bits/newCheckout/made", json!({"who": "terminal ab"}));
    assert_eq!(made["made"], true);
    assert_eq!(made["made_by"], "terminal ab");
    let d = b.get(&format!("goals/G{g}"));
    assert_eq!(d["bits_waiting"], 0);
    assert!(d["finished_at"].is_string());
    b.post("bits/newCheckout/made", json!({"undo": true}));
    let mut q = Query::new();
    q.insert("goal".into(), format!("G{g}"));
    assert_eq!(api::dispatch(&b.app, "GET", "bits", &q, &json!({})).unwrap()["bits"][1]["made"], false);
    assert_eq!(b.get("bits")["tool"], "Flagsmith");
    b.post("bits/beta-banner/remove", json!({}));
    assert_eq!(b.get("bits")["bits"].as_array().unwrap().len(), 1);
}

#[test]
fn only_the_app_sets_a_review_stop() {
    let b = new_board();
    let g = b.goal();
    let t = b.task("One", json!({"goal_id": g, "wave": 1}));
    let (code, msg) = b.post_err(&format!("goals/G{g}/waves/1"), json!({"stop_after": true}));
    assert_eq!(code, 403);
    assert!(msg.contains("Only the owner"), "{msg}");
    b.post(&format!("goals/G{g}/waves/1"), json!({"name": "Basics"}));
    let d = api::dispatch(&b.app, "POST", &format!("goals/G{g}/waves/1"), &app_query(), &json!({"stop_after": true})).unwrap();
    assert_eq!(d["waves"][0]["stop_after"], true);
    assert_eq!(d["waves"][0]["name"], "Basics");
    let _ = t;
}

#[test]
fn a_held_wave_waits_and_so_does_everything_after_it_until_continued() {
    let b = new_board();
    let g = b.goal();
    let one = b.task("One", json!({"goal_id": g, "wave": 1}));
    let two = b.task("Two", json!({"goal_id": g, "wave": 2}));
    b.post(&format!("goals/G{g}/run"), json!({}));
    let d = b.post(&format!("goals/G{g}/waves/1/hold"), json!({"who": "Planner"}));
    assert_eq!(d["waves"][0]["state"], "held");
    assert_eq!(d["waves"][0]["held"], true);
    assert_eq!(b.waiting(one), "Wave 1 is held until it's continued");
    assert_eq!(b.waiting(two), "Wave 1 is held until it's continued");
    runner::start_queued(&b.app).unwrap();
    assert!(b.started().is_empty());
    assert_eq!(b.get(&format!("goals/G{g}"))["held"], 2);

    assert_eq!(b.post(&format!("goals/G{g}/waves/1/continue"), json!({}))["let_start"], true);
    let w = b.get(&format!("goals/G{g}"))["waves"].clone();
    assert_eq!(w[0]["state"], "ready");
    assert!(w[0]["released_at"].is_null(), "continuing a held wave lets it start, it doesn't skip it");
    assert_eq!(w[1]["state"], "waiting");
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![one]);

    b.post(&format!("goals/G{g}/waves/2/hold"), json!({}));
    b.post(&format!("goals/G{g}/waves/2/hold"), json!({"on": false}));
    assert_eq!(b.get(&format!("goals/G{g}"))["waves"][1]["held"], false);
    b.set(one, "done");
    b.set(two, "done");
    let (code, _) = b.post_err(&format!("goals/G{g}/waves/2/hold"), json!({}));
    assert_eq!(code, 409);
}

#[test]
fn tb_take_lends_devices_and_they_come_back_when_the_task_ends() {
    let b = new_board();
    let repo = b._dir.path().join("webapp");
    taskboardd::midna::sync(&b.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    b.post("devices", json!({"name": "iphone-15", "tags": "ios", "note": "iOS 18"}));
    let t = b.task("Sim test", json!({"devices": "ios"}));
    let r = taskboardd::reports::handle(
        &b.app,
        json!({"event": "tb.take", "session": "s1", "claude_session": "c-s1", "cwd": "", "task": format!("T{t}")}),
        false,
    )
    .unwrap_or_else(|e| panic!("tb.take: {}", e.message));
    assert_eq!(b.get(&format!("tasks/T{t}"))["devices"]["lent"], json!(["iphone-15"]));
    let ctx = r.to_string();
    assert!(ctx.contains("The board lent this task the device iphone-15"), "{ctx}");
    assert_eq!(b.get("devices")["devices"][0]["held_by"]["ref"], format!("T{t}"));

    b.set(t, "done");
    runner::tick(&b.app).unwrap();
    assert!(b.get("devices")["devices"][0]["held_by"].is_null(), "the device came back");
}

#[test]
fn tb_task_new_with_an_unknown_bit_adds_no_task() {
    let b = new_board();
    let repo = b._dir.path().join("webapp");
    taskboardd::midna::sync(&b.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    let g = b.goal();
    let report = |extra: Value| {
        let mut body = json!({"event": "tb.new_task", "session": "s1", "claude_session": "c-s1", "cwd": "", "title": "Checkout", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        taskboardd::reports::handle(&b.app, body, false)
    };
    let e = report(json!({"bits": ["missing"]})).expect_err("an unknown bit");
    assert_eq!(e.status, 404, "{}", e.message);
    let e = report(json!({"goal": format!("G{g}"), "bits": ["missing"]})).expect_err("an unknown bit in a goal");
    assert_eq!(e.status, 404, "{}", e.message);
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM tasks", p![]).unwrap(), 0, "nothing was added");

    b.post("bits", json!({"name": "newCheckout", "kind": "backend"}));
    let v = report(json!({"goal": format!("G{g}"), "bits": ["newCheckout"], "devices": "ios"})).unwrap();
    let r = v["created"][0].as_str().unwrap().to_string();
    let t = b.get(&format!("tasks/{r}"));
    assert_eq!(t["bits"][0]["name"], "newCheckout");
    assert_eq!(t["devices"]["needs_text"], "ios");
    let v = report(json!({"goal": format!("G{g}")})).unwrap();
    let r = v["created"][0].as_str().unwrap().to_string();
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM device_needs WHERE owner = ?", p![r]).unwrap(), 0, "no --device sets no needs");
}

#[test]
fn a_task_can_need_no_devices_over_its_goals_needs() {
    let b = new_board();
    let g = b.goal();
    b.post(&format!("goals/G{g}"), json!({"devices": "ios"}));
    let t = b.task("Docs only", json!({"goal_id": g}));
    assert_eq!(b.get(&format!("tasks/T{t}"))["devices"]["needs_text"], "ios");
    b.post(&format!("tasks/T{t}"), json!({"devices": "none"}));
    assert!(b.get(&format!("tasks/T{t}"))["devices"].is_null(), "its own none wins over the goal's ios");
    b.post(&format!("goals/G{g}/run"), json!({}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![t], "it needs no device, so the empty pool doesn't hold it");
    b.post(&format!("tasks/T{t}"), json!({"devices": "goal"}));
    assert_eq!(b.get(&format!("tasks/T{t}"))["devices"]["needs_text"], "ios", "goal: back to the goal's needs");
    b.post(&format!("goals/G{g}"), json!({"devices": "none"}));
    assert_eq!(b.app.db.count("SELECT COUNT(*) FROM device_needs WHERE owner LIKE 'G%'", p![]).unwrap(), 0);
}

#[test]
fn a_task_started_by_hand_without_free_devices_says_so() {
    let b = new_board();
    b.post("devices", json!({"name": "pixel-7", "tags": "android"}));
    let a = b.task("A", json!({"devices": "android"}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![a]);
    let c = b.task("C", json!({"devices": "android"}));
    b.post(&format!("tasks/T{c}/start"), json!({"mode": "new"}));
    assert_eq!(b.get(&format!("tasks/T{c}"))["devices"]["lent"], json!([]));
    let want = format!("Waits for a android device (T{a} has them); it started without");
    let said = b.app.db.count("SELECT COUNT(*) FROM events WHERE task_id = ? AND text = ?", p![c, want]).unwrap();
    assert_eq!(said, 1, "{:?}", b.app.db.q("SELECT text FROM events WHERE task_id = ?", p![c]).unwrap());
    // Switched off while lent: the task keeps it.
    let d = b.post("devices/pixel-7", json!({"off": true}));
    assert_eq!((d["off"].clone(), d["held_by"]["ref"].clone()), (json!(true), json!(format!("T{a}"))));
}

#[test]
fn a_goal_keeps_its_own_devices() {
    let b = new_board();
    b.post("devices", json!({"name": "pixel-7", "tags": "android"}));
    b.post("devices", json!({"name": "pixel-8", "tags": "android"}));
    b.post("devices", json!({"name": "rig", "tags": "bench"}));
    let g3 = b.goal();
    let g4 = b.goal();
    // G3 reserves pixel-8 and gives the rig its purpose.
    let pool = b.post(&format!("goals/G{g3}/devices"), json!({"device": "pixel-8", "reserved": true}));
    assert_eq!(pool["devices"][0]["reserved"], true);
    b.post(&format!("goals/G{g3}/devices"), json!({"device": "rig", "purpose": "measure"}));
    // Two goals may reserve one device, as on the Python board: they share it.
    b.post(&format!("goals/G{g4}/devices"), json!({"device": "pixel-8", "reserved": true}));
    assert_eq!(b.get("devices/pixel-8")["reserved_for"], format!("G{g3} and G{g4}"));
    b.post(&format!("goals/G{g4}/devices/pixel-8/remove"), json!({}));
    let (code, _) = b.post_err(&format!("goals/G{g3}/devices"), json!({"device": "rig", "purpose": "Not A Tag!"}));
    assert_eq!(code, 400);

    // Another goal's task, and a task in no goal, never get pixel-8.
    let other = b.task("Other", json!({"goal_id": g4, "devices": "android:2"}));
    b.post(&format!("goals/G{g4}/run"), json!({}));
    assert_eq!(b.waiting(other), format!("Needs 2 android devices, and the pool has 1 (pixel-8 is reserved for G{g3})"));
    b.post(&format!("tasks/T{other}"), json!({"devices": "android"}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.get(&format!("tasks/T{other}"))["devices"]["lent"], json!(["pixel-7"]));
    let loose = b.task("Loose", json!({"devices": "android"}));
    let why = b.waiting(loose).as_str().unwrap_or("").to_string();
    assert_eq!(why, format!("Waits for a android device (T{other} has them)"), "pixel-8 isn't its to wait for");
    b.post("devices/pixel-7", json!({"off": true}));
    let why = b.waiting(loose).as_str().unwrap_or("").to_string();
    assert_eq!(why, format!("Needs a android device, and the pool has none for it (pixel-8 is reserved for G{g3})"));

    // G3's task gets its own pool first, and the purpose counts as a tag there only.
    let mine = b.task("Mine", json!({"goal_id": g3, "devices": "android measure"}));
    b.post(&format!("goals/G{g3}/run"), json!({}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.get(&format!("tasks/T{mine}"))["devices"]["lent"], json!(["pixel-8", "rig"]));
    let h = handoff::build(&b.app, mine).unwrap();
    assert!(h.contains("rig (bench, measure)"), "{h}");
    let outside = b.task("Measure elsewhere", json!({"goal_id": g4, "devices": "measure"}));
    assert_eq!(b.waiting(outside), "Needs a measure device, and the pool has none (tb device add)");

    let d = b.get(&format!("goals/G{g3}"));
    assert_eq!(d["devices"]["pool"], 2);
    assert_eq!(d["devices"]["devices"][0]["in_pool"], true);
    assert_eq!(d["devices"]["devices"][2]["in_pool"], false, "the goal's own devices come first");
    assert_eq!(b.get("devices/pixel-8")["reserved_for"], format!("G{g3}"));
    b.post(&format!("goals/G{g3}/devices/pixel-8/remove"), json!({}));
    assert!(b.get("devices/pixel-8")["reserved_for"].is_null());
    let (code, _) = b.post_err(&format!("goals/G{g3}/devices/pixel-8/remove"), json!({}));
    assert_eq!(code, 404);
}

/// The Python board's pool rules (#84): a goal with devices of its own lends only those, a purpose
/// can be a comma list, and an archived goal holds nothing back.
#[test]
fn a_goals_pool_lends_only_its_own_and_an_archived_goal_lets_go() {
    let b = new_board();
    b.post("devices", json!({"name": "pixel-7", "tags": "android"}));
    b.post("devices", json!({"name": "pixel-8", "tags": "android"}));
    let g3 = b.goal();
    let g4 = b.goal();
    let pool = b.post(&format!("goals/G{g3}/devices"), json!({"device": "pixel-8", "purpose": "measure, demo", "reserved": true}));
    assert_eq!(pool["devices"][0]["purpose"], "measure,demo");

    // G3's tasks get only G3's devices, each purpose a tag.
    let two = b.task("Two", json!({"goal_id": g3, "devices": "android:2"}));
    b.post(&format!("goals/G{g3}/run"), json!({}));
    assert_eq!(b.waiting(two), format!("Needs 2 android devices, and G{g3}'s own devices have 1"));
    b.post(&format!("tasks/T{two}"), json!({"devices": "demo"}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.get(&format!("tasks/T{two}"))["devices"]["lent"], json!(["pixel-8"]));
    b.set(two, "done");

    // Archived, G3 reserves nothing: G4's task can have pixel-8, and the device says nothing of G3.
    let other = b.task("Other", json!({"goal_id": g4, "devices": "android:2"}));
    b.post(&format!("goals/G{g4}/run"), json!({}));
    assert!(b.waiting(other).as_str().unwrap_or("").contains(&format!("reserved for G{g3}")));
    b.post(&format!("goals/G{g3}"), json!({"archived": true}));
    assert!(b.get("devices/pixel-8")["reserved_for"].is_null());
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.get(&format!("tasks/T{other}"))["devices"]["lent"], json!(["pixel-7", "pixel-8"]));
    assert_eq!(b.get(&format!("goals/G{g3}/devices"))["devices"][0]["name"], "pixel-8", "its page still lists its own");
}

/// Regression (#86): a device asked for by name is lent whatever the goal's pool, as on the Python
/// board; only tag needs keep to the goal's own devices.
#[test]
fn a_named_device_outside_the_goals_pool_is_lent() {
    let b = new_board();
    b.post("devices", json!({"name": "pixel-7", "tags": "android"}));
    b.post("devices", json!({"name": "pixel-8", "tags": "android"}));
    let g3 = b.goal();
    b.post(&format!("goals/G{g3}/devices"), json!({"device": "pixel-8"}));
    let named = b.task("Named", json!({"goal_id": g3, "devices": "pixel-7"}));
    b.post(&format!("goals/G{g3}/run"), json!({}));
    assert!(b.waiting(named).is_null(), "{}", b.waiting(named));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.get(&format!("tasks/T{named}"))["devices"]["lent"], json!(["pixel-7"]));
    // A tag still keeps to G3's own: pixel-7 doesn't count as an android device for it.
    let two = b.task("Two", json!({"goal_id": g3, "devices": "android:2"}));
    assert_eq!(b.waiting(two), format!("Needs 2 android devices, and G{g3}'s own devices have 1"));
}

/// Regression (#91): a named device reserved for another goal, or a name the pool doesn't have, says
/// so before the goal's own pool does, as on the Python board.
#[test]
fn a_reserved_or_missing_named_device_says_why_before_the_goals_pool() {
    let b = new_board();
    b.post("devices", json!({"name": "dev-a", "tags": "android"}));
    b.post("devices", json!({"name": "dev-c", "tags": "android"}));
    let g1 = b.goal();
    let g2 = b.goal();
    b.post(&format!("goals/G{g1}/devices"), json!({"device": "dev-a"}));
    b.post(&format!("goals/G{g2}/devices"), json!({"device": "dev-c", "reserved": true}));
    let named = b.task("Named", json!({"goal_id": g1, "devices": "dev-c"}));
    let missing = b.task("Missing", json!({"goal_id": g1, "devices": "dev-zz"}));
    b.post(&format!("goals/G{g1}/run"), json!({}));
    assert_eq!(b.waiting(named), format!("Waiting for a free dev-c (dev-c is reserved for G{g2})"));
    assert_eq!(b.waiting(missing), "No dev-zz yet (tb device add)");
    runner::start_queued(&b.app).unwrap();
    assert!(b.started().is_empty());
    // A task in no goal gets the same words.
    let loose = b.task("Loose", json!({"devices": "dev-c"}));
    assert_eq!(b.waiting(loose), format!("Waiting for a free dev-c (dev-c is reserved for G{g2})"));
    // Switched off, a named device is waited for by name, not as the goal's pool.
    b.post("devices/dev-a", json!({"off": true}));
    let off = b.task("Off", json!({"goal_id": g1, "devices": "dev-a"}));
    b.post(&format!("goals/G{g1}/run"), json!({}));
    assert_eq!(b.waiting(off), "Waiting for dev-a (it's off)");
}

/// #98: a named device another task has is waited for by name, as on the Python board.
#[test]
fn a_named_device_another_task_has_says_who_has_it() {
    let b = new_board();
    b.post("devices", json!({"name": "dev-a", "tags": "android"}));
    let holder = b.task("Holder", json!({"devices": "dev-a"}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.started(), vec![holder]);
    let named = b.task("Named", json!({"devices": "dev-a"}));
    assert_eq!(b.waiting(named), format!("Waiting for a free dev-a (dev-a is with T{holder})"));
    // The tag still reads as a tag.
    let tagged = b.task("Tagged", json!({"devices": "android"}));
    assert_eq!(b.waiting(tagged), format!("Waits for a android device (T{holder} has them)"));
}

#[test]
fn with_goal_pool_only_off_a_goal_borrows_from_the_rest() {
    let b = board_with(|c| c.devices.goal_pool_only = false);
    b.post("devices", json!({"name": "pixel-7", "tags": "android"}));
    b.post("devices", json!({"name": "pixel-8", "tags": "android"}));
    let g3 = b.goal();
    b.post(&format!("goals/G{g3}/devices"), json!({"device": "pixel-8"}));
    let two = b.task("Two", json!({"goal_id": g3, "devices": "android:2"}));
    b.post(&format!("goals/G{g3}/run"), json!({}));
    runner::start_queued(&b.app).unwrap();
    assert_eq!(b.get(&format!("tasks/T{two}"))["devices"]["lent"], json!(["pixel-8", "pixel-7"]), "its own first");
}
