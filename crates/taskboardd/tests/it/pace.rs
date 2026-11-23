//! The plan check's pace warnings: a long line of tasks that wait on each other (shared PRs, stacks and
//! waits), a plan spread over many waves or a change that grows them, far more open tasks than the goal's
//! terminal limit, and several tasks that need one device by name.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, devices, midna, pace, reports};

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

fn strs(v: &Value) -> Vec<String> {
    v.as_array().cloned().unwrap_or_default().iter().filter_map(|w| w.as_str().map(|s| s.to_string())).collect()
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn goal(&self, extra: Value) -> i64 {
        let mut body = json!({"name": "Mac app", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/goals", body)["id"].as_i64().unwrap()
    }
    /// Proposes `tasks` for goal `g`: (the tasks made, as ids; the plan check's warnings).
    fn propose(&self, g: i64, tasks: Value) -> (Vec<i64>, Vec<String>) {
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": self.dir.path().join("webapp").to_string_lossy(), "status": {"state": "working"}})], &[])
            .unwrap();
        let b = crate::est::fill(json!({"event": "tb.propose", "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}, "goal": format!("G{g}"), "tasks": tasks}));
        let r = reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("propose: {}", e.message));
        (strs(&r["created"]).iter().map(|t| t.trim_start_matches('T').parse().unwrap()).collect(), strs(&r["warnings"]))
    }
    fn warnings(&self, g: i64) -> Vec<String> {
        pace::plan_warnings(&self.app, &board::get_goal(&self.app, g).unwrap(), None).unwrap()
    }
}

/// `n` tasks in waves `from`, `from + 1`, … each on its own file.
fn waved(n: usize, from: usize) -> Value {
    json!((0..n).map(|i| json!({"title": format!("Fix {i}"), "detail": "Fix the spacing.\nMatch the design.", "wave": from + i, "files": [format!("src/s{}{i}.rs", from)]})).collect::<Vec<_>>())
}

fn find(w: &[String], bit: &str) -> Option<String> {
    w.iter().find(|w| w.contains(bit)).cloned()
}

#[test]
fn a_line_of_more_than_five_tasks_sharing_a_pr_warns_and_says_to_fold_them() {
    let b = new_board();
    let g = b.goal(json!({}));
    let (ids, _) = b.propose(g, waved(6, 1));
    let line = |w: &[String]| find(w, "run one after another");
    for (i, id) in ids.iter().enumerate().skip(1).take(4) {
        let w = strs(&b.post(&format!("/tasks/{id}"), json!({"same_pr": format!("T{}", ids[0])}))["warnings"]);
        assert_eq!(line(&w), None, "{} in a line is fine: {w:?}", i + 1);
    }
    let w = strs(&b.post(&format!("/tasks/{}", ids[5]), json!({"same_pr": format!("T{}", ids[0])}))["warnings"]);
    let text = line(&w).unwrap_or_else(|| panic!("the sixth makes a line: {w:?}"));
    assert!(text.starts_with(&format!("6 tasks run one after another, each waiting for the one before (shared PRs, stacks and waits): T{}, T{}, T{}, T{}, T{}, T{}.",
                                      ids[0], ids[1], ids[2], ids[3], ids[4], ids[5])), "{text}");
    assert!(text.contains("one task, with a checklist of the backlog items it fixes") && text.contains("--issue B<n>"), "{text}");

    // Plain waits make a line too; tasks that don't wait on each other don't.
    let g2 = b.goal(json!({}));
    let (side, _) = b.propose(g2, json!((0..8).map(|i| json!({"title": format!("Screen {i}"), "detail": "Fix it.\nAll of it.", "wave": 1, "files": [format!("src/w{i}.rs")]})).collect::<Vec<_>>()));
    assert_eq!(line(&b.warnings(g2)), None, "eight side by side");
    for pair in side.windows(2) {
        b.post(&format!("/tasks/{}", pair[1]), json!({"waits_for": format!("T{}", pair[0])}));
    }
    let text = line(&b.warnings(g2)).unwrap();
    assert!(text.starts_with(&format!("8 tasks run one after another, each waiting for the one before (shared PRs, stacks and waits): T{}, T{}, T{} … T{}, T{}, T{}.",
                                      side[0], side[1], side[2], side[5], side[6], side[7])), "{text}");
}

#[test]
fn many_waves_or_a_change_that_grows_them_warns() {
    let b = new_board();
    let waves = |w: &[String]| find(w, "waves still to run");
    let g = b.goal(json!({"max_terminals": 4}));
    let (_, w) = b.propose(g, waved(12, 1));
    assert_eq!(waves(&w), None, "twelve is fine: {w:?}");
    let (_, w) = b.propose(g, waved(1, 13));
    assert_eq!(waves(&w).as_deref(), Some(
        "The plan has 13 waves still to run (it had 12 before this change). Tasks that share a PR go in different waves, so a line of them adds a wave each: \
         put small findings on the same screen or area in one task, with a checklist of the backlog items it fixes (tb task new … --issue B<n> for each), \
         not a line of tasks sharing a PR, and let the screens run side by side."
    ));

    let g = b.goal(json!({"max_terminals": 4}));
    b.propose(g, waved(3, 1));
    let (_, w) = b.propose(g, waved(5, 4));
    assert_eq!(waves(&w), None, "3 to 8 grows by less than six: {w:?}");
    let (_, w) = b.propose(g, waved(1, 9));
    assert_eq!(waves(&w), None, "8 to 9 is neither past twelve nor doubled: {w:?}");

    let g = b.goal(json!({"max_terminals": 4}));
    b.propose(g, waved(4, 1));
    let (_, w) = b.propose(g, waved(6, 5));
    assert!(waves(&w).is_some_and(|l| l.starts_with("The plan has 10 waves still to run (it had 4 before this change).")), "4 to 10 in one re-plan: {w:?}");
}

#[test]
fn far_more_open_tasks_than_the_terminal_limit_warns() {
    let b = new_board();
    let cap = |w: &[String]| find(w, "so that limit sets its pace");
    let g = b.goal(json!({}));
    let many = |n: usize| json!((0..n).map(|i| json!({"title": format!("Fix {i}"), "detail": "Fix it.\nAll of it.", "files": [format!("src/c{i}.rs")]})).collect::<Vec<_>>());
    let (_, w) = b.propose(g, many(16));
    assert_eq!(cap(&w), None, "eight a terminal at the default of two: {w:?}");
    let (_, w) = b.propose(g, many(1));
    assert_eq!(cap(&w), Some(format!(
        "G{g} has 17 open tasks and runs at most 2 terminals at a time, so that limit sets its pace: fold small findings into one task per screen or area, \
         and tell the owner, who can raise it (tb goal set G{g} --max-terminals <n>)."
    )));
    b.post(&format!("/goals/{g}"), json!({"max_terminals": 3}));
    assert_eq!(cap(&b.warnings(g)), None, "three terminals take 24");
}

#[test]
fn several_tasks_that_need_one_device_by_name_warn_and_name_its_tags() {
    let b = new_board();
    b.post("devices", json!({"name": "sim-a", "tags": "ios iphone"}));
    b.post("devices", json!({"name": "sim-b", "tags": "ios"}));
    b.post("devices", json!({"name": "rig", "tags": ""}));
    let g = b.goal(json!({}));
    let one = |title: &str, device: &str| json!({"title": title, "detail": "Check it.\nOn the phone.", "devices": device, "files": [format!("src/{title}.rs")]});
    let (ids, w) = b.propose(g, json!([one("search", "sim-a"), one("settings", "ios"), one("drafts", "iphone"), one("bench", "rig")]));
    let dev = |w: &[String]| w.iter().filter(|w| w.contains("by name")).cloned().collect::<Vec<_>>();
    assert_eq!(dev(&w), Vec::<String>::new(), "one task a name, and tags: {w:?}");

    let (more, w) = b.propose(g, json!([one("reply", "sim-a"), one("bench2", "rig")]));
    assert_eq!(dev(&w), vec![
        format!("T{}, T{} all need the device rig by name, so they run one at a time behind it however many like it the pool has: \
                 ask for a tag (tb device set rig --tag <tag> on the owner's word, then --device <tag>), unless only rig will do.", ids[3], more[1]),
        format!("T{}, T{} all need the device sim-a by name, so they run one at a time behind it however many like it the pool has: \
                 ask for one of its tags (ios, iphone; tb task set T{} --device iphone), unless only sim-a will do.", ids[0], more[0], ids[0]),
    ]);
    let goal = board::get_goal(&b.app, g).unwrap();
    b.post(&format!("/tasks/{}", ids[0]), json!({"devices": "iphone"}));
    assert_eq!(devices::plan_warnings(&b.app, &goal).unwrap().len(), 1, "asked by tag now: only rig's pair is left");
}
