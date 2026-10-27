//! The board closes a terminal in a worktree slot with Midna's close cleanup off, and leaves a slot's
//! agent terminal open on exit for that close: Midna's cleanup would remove the slot's worktree.
//! Every slot's path also goes in Midna's `cleanup.keep`, for a terminal elsewhere that worked in it.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::{json, Value};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, p};

struct Board {
    app: std::sync::Arc<App>,
    dir: tempfile::TempDir,
    slot: String,
    plain: String,
}

/// A board whose Midna logs each call, says S-slot runs in a slot and S-plain in the repo, and lists
/// S-slot as a board tab whose Claude exited. `runner` turns on the sync's closing of exited tabs.
fn new_board(runner: bool) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("webapp").to_string_lossy().to_string();
    let slot = format!("{plain}/.claude/worktrees/slot-1");
    std::fs::create_dir_all(&slot).unwrap();
    let exited = json!([{"id": "S-slot", "name": "T1 Fix", "agent": "", "cwd": slot, "status": {"state": "idle", "reason": "claude exited"}}]);
    let exe = dir.path().join("fake-midna");
    std::fs::write(
        &exe,
        format!(
            "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s %s\\n' \"$2\" \"$3\" >> \"$d/calls\"\ncase \"$2 $3\" in\n  session.open*) echo '{{\"id\": \"S9\"}}' ;;\n  session.get*S-slot*) echo '{{\"cwd\": \"{slot}/crates\"}}' ;;\n  session.get*) echo '{{\"cwd\": \"{plain}\"}}' ;;\n  session.list*) echo '{exited}' ;;\n  project.list*|events.list*) echo '[]' ;;\n  settings.get*cleanup.keep*) echo '{{\"value\": [\"main\"]}}' ;;\n  *) echo '{{}}' ;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.midna = exe;
    cfg.runner = runner;
    Board { app: App::for_tests(cfg), dir, slot, plain }
}

impl Board {
    fn calls(&self, method: &str) -> Vec<Value> {
        let calls = std::fs::read_to_string(self.dir.path().join("calls")).unwrap_or_default();
        calls
            .lines()
            .filter_map(|l| l.split_once(' '))
            .filter(|(m, _)| *m == method)
            .map(|(_, p)| serde_json::from_str(p).unwrap())
            .collect()
    }
    fn run(&self, kind: &str, args: Value) {
        let id = board::create_job(&self.app, kind, args, None, "", None).unwrap();
        let j = self.app.db.q1("SELECT * FROM jobs WHERE id = ?", p![id]).unwrap().unwrap();
        midna::run_job(&self.app, &j);
        let j = self.app.db.q1("SELECT * FROM jobs WHERE id = ?", p![id]).unwrap().unwrap();
        assert_eq!(j.s("state"), Some("done"), "{kind} job: {j:?}");
    }
}

#[test]
fn closing_a_slot_s_terminal_turns_midna_s_cleanup_off() {
    let b = new_board(false);
    b.run("close", json!({"session": "S-slot"}));
    b.run("close", json!({"session": "S-plain"}));
    let closes = b.calls("session.close");
    assert_eq!(closes.len(), 2, "{closes:?}");
    assert_eq!((closes[0]["id"].clone(), closes[0]["cleanup"].clone()), (json!("S-slot"), json!(false)));
    assert_eq!(closes[1]["id"], "S-plain");
    assert!(closes[1].get("cleanup").is_none(), "a terminal outside the pool keeps Midna's cleanup: {closes:?}");
}

#[test]
fn a_slot_s_agent_terminal_is_left_for_the_board_to_close() {
    let b = new_board(false);
    for cwd in [&b.slot, &b.plain] {
        b.run("agent", json!({"cwd": cwd, "prompt": "go", "title": "Fix"}));
    }
    let opens = b.calls("session.open");
    assert_eq!(opens.len(), 2, "{opens:?}");
    assert_eq!((opens[0]["cwd"].as_str(), opens[0]["close_on_exit"].clone()), (Some(b.slot.as_str()), json!(false)));
    assert_eq!((opens[1]["cwd"].as_str(), opens[1]["close_on_exit"].clone()), (Some(b.plain.as_str()), json!(true)));
}

#[test]
fn a_slot_tab_whose_claude_exited_closes_with_cleanup_off() {
    let b = new_board(true);
    let working = json!({"id": "S-slot", "name": "T1 Fix", "agent": "claude", "cwd": b.slot, "status": {"state": "working"}});
    midna::sync(&b.app, &[working], &[]).unwrap();
    midna::sync_once(&b.app).unwrap();
    let closes = b.calls("session.close");
    assert_eq!(closes, [json!({"id": "S-slot", "force": false, "cleanup": false})], "{closes:?}");
    assert!(Path::new(&b.slot).is_dir());
}

#[test]
fn the_pool_s_slots_go_in_midna_s_cleanup_keep_once() {
    let b = new_board(false);
    b.app.db.insert("worktree_slots", vec![("repo", json!(b.plain)), ("n", json!(1)), ("path", json!(b.slot)), ("made_at", json!("2026-10-27T19:47:33Z"))]).unwrap();
    midna::sync_once(&b.app).unwrap();
    midna::sync_once(&b.app).unwrap();
    let sets: Vec<Value> = b.calls("settings.set").into_iter().filter(|c| c["key"] == "cleanup.keep").collect();
    assert_eq!(sets, [json!({"key": "cleanup.keep", "value": ["main", b.slot]})], "{sets:?}");
    let gets = b.calls("settings.get").into_iter().filter(|c| c["key"] == "cleanup.keep").count();
    assert_eq!(gets, 1, "a sync with the same slots asks Midna nothing");
}
