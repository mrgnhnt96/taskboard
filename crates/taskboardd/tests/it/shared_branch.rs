//! `GET /slots/sharing`: the other terminals working in a main checkout now, so the hook can refuse a
//! branch move under them. A terminal counts while working or waiting on the owner, by its task's
//! checkout, else by its folder; one in a worktree nested in the checkout doesn't.

use serde_json::Value;
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{fields, slots};

#[test]
fn only_live_terminals_in_the_checkout_itself_share_it() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let repo = dir.path().join("webapp");
    let slot = repo.join(".claude/worktrees/slot-1");
    let other = dir.path().join("api");
    for d in [&repo.join("crates"), &slot, &other] {
        std::fs::create_dir_all(d).unwrap();
    }
    let (repo, slot, other) = (repo.to_string_lossy().to_string(), slot.to_string_lossy().to_string(), other.to_string_lossy().to_string());
    let session = |id: &str, name: &str, status: &str, cwd: &str| {
        app.db.insert("sessions", fields!["id" => id, "name" => name, "status" => status, "cwd" => cwd, "project_path" => cwd]).unwrap();
    };
    session("me", "Mine", "working", &repo);
    session("tab", "Morgan's tab", "working", &format!("{repo}/crates"));
    session("asks", "Asks", "needs", &repo);
    session("idle", "Idle", "idle", &repo);
    session("slotted", "Slotted", "working", &slot);
    session("elsewhere", "Elsewhere", "working", &other);
    // A task's checkout wins over the terminal's folder: this one started in the repo, works in the slot.
    session("moved", "Moved", "working", &repo);
    app.db.insert("tasks", fields!["title" => "x", "project" => "webapp", "status" => "working", "session_id" => "moved", "repo_path" => slot.as_str()]).unwrap();
    session("ontask", "On a task", "working", &other);
    app.db.insert("tasks", fields!["title" => "y", "project" => "webapp", "status" => "working", "session_id" => "ontask", "repo_path" => repo.as_str()]).unwrap();

    let v = slots::sharing(&app, &repo, "me").unwrap();
    let names: Vec<&str> = v["sharing"].as_array().unwrap().iter().filter_map(|o| o["name"].as_str()).collect();
    assert_eq!(names, vec!["Asks", "On a task", "Morgan's tab"], "{v}");
    let ontask = v["sharing"].as_array().unwrap().iter().find(|o| o["session"] == "ontask").unwrap();
    assert!(ontask["task"].as_str().is_some_and(|t| t.starts_with('T')), "{ontask}");
    assert_eq!(slots::sharing(&app, &slot, "slotted").unwrap()["sharing"].as_array().map(Vec::len), Some(0), "a slot's own terminal is alone there");
    assert_eq!(slots::sharing(&app, "", "me").unwrap()["sharing"], Value::Array(vec![]));
}
