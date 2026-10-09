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
    assert_eq!(b.waiting(t), "Needs a ios device, and the pool has none (tb device add)");
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
fn a_task_waits_on_its_backend_bits_and_a_goal_waits_on_them_after_its_tasks() {
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

    b.post(&format!("goals/G{g}/run"), json!({}));
    assert_eq!(b.waiting(t), "Waits for the bit newCheckout to be made in Flagsmith");
    runner::start_queued(&b.app).unwrap();
    assert!(b.started().is_empty());
    let d = b.get(&format!("goals/G{g}"));
    assert_eq!(d["bits"]["backend"], 1);
    assert_eq!(d["bits"]["made"], 0);
    assert_eq!(d["bits"]["list"].as_array().unwrap().len(), 2);
    let h = handoff::build(&b.app, t).unwrap();
    assert!(h.contains("beta-banner (local: in the code only, not in Flagsmith), newCheckout (backend: not made in Flagsmith yet)"), "{h}");

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

    b.post(&format!("goals/G{g}/waves/1/continue"), json!({}));
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
