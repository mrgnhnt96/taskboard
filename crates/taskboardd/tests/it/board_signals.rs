//! What the app's pages read to show one stop state, a task's terminal and blockers, shared tasks and
//! the Backlog page's groups: a goal's `stop` (review, failed or held) on its dict and Home's stage,
//! the record of passing a failed wave, a card's `terminal_closed_at` and its waits' PRs, a goal's
//! `shared_tasks` with the goal each came from, and the triage list's true group totals.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, fields, midna, p, reports};

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
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        let (p, q) = path.split_once('?').unwrap_or((path, ""));
        let query: Query = q.split('&').filter(|x| !x.is_empty()).filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        api::dispatch(&self.app, "GET", p, &query, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn set(&self, id: i64, status: &str, failed: bool) {
        board::update_task(&self.app, id, fields!["status" => status, "failed" => failed as i64]).unwrap();
    }
    fn card(&self, t: i64) -> Value {
        board::task_card(&self.app, &board::get_task(&self.app, t).unwrap()).unwrap()
    }
    fn goal(&self, g: i64) -> Value {
        board::goal_dict(&self.app, &board::get_goal(&self.app, g).unwrap()).unwrap()
    }
    /// The goal's entry on Home.
    fn home_entry(&self, g: i64) -> Value {
        let h = self.get("home");
        let all = h.as_object().unwrap().values().filter_map(Value::as_array).flatten().cloned().collect::<Vec<_>>();
        all.into_iter().find(|e| e["goal"]["id"].as_i64() == Some(g)).unwrap_or_else(|| panic!("G{g} on Home: {h}"))
    }
    fn task(&self, body: Value) -> i64 {
        let mut b = json!({"title": "A task", "detail": "Do it.", "project": "webapp"});
        merge(&mut b, body);
        self.post("/tasks", b)["id"].as_i64().unwrap()
    }
    fn new_goal(&self, name: &str) -> i64 {
        self.post("/goals", json!({"name": name, "project": "webapp"}))["id"].as_i64().unwrap()
    }
}

fn merge(a: &mut Value, b: Value) {
    for (k, v) in b.as_object().unwrap() {
        a[k] = v.clone();
    }
}

/// A goal planned through `tb propose` with two tasks in wave 1 and one in wave 2, then run.
fn planned(b: &Board) -> (i64, Vec<i64>) {
    midna::sync(&b.app, &[json!({"id": "s1", "name": "Planner", "agent": "claude", "cwd": b.dir.path().join("webapp").to_string_lossy(), "status": {"state": "idle"}})], &[]).unwrap();
    let g = b.post("/goals", json!({"name": "Login", "project": "webapp", "run_in_order": true, "max_terminals": 2}))["id"].as_i64().unwrap();
    let tasks = ["Model::add it::1", "Form::build it::1", "Wire::connect them::2"];
    let r = reports::handle(&b.app, json!({"event": "tb.propose", "session": "s1", "goal": format!("G{g}"), "tasks": tasks}), false).unwrap();
    let ids: Vec<i64> = r["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()[1..].parse().unwrap()).collect();
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/run"), &[(api::FROM.to_string(), "app".to_string())].into_iter().collect::<Query>(), &json!({})).unwrap();
    (g, ids)
}

#[test]
fn a_failed_wave_is_a_stop_and_passing_it_is_recorded() {
    let b = new_board();
    let (g, t) = planned(&b);
    b.set(t[0], "done", true);
    b.set(t[1], "done", false);
    let stop = b.goal(g)["stop"].clone();
    assert_eq!(stop["kind"], "failed", "{stop}");
    assert_eq!(stop["wave"], 1);
    assert_eq!(stop["tasks"], json!([format!("T{}", t[0])]));
    assert_eq!(stop["line"], b.goal(g)["stopped"], "the same line the goal's `stopped` shows");
    let stage = b.home_entry(g)["stage"].clone();
    assert_eq!((stage["kind"].as_str(), stage["wave"].as_i64()), (Some("failed"), Some(1)), "{stage}");
    assert_eq!(stage["failed"], json!([format!("T{}", t[0])]));

    let d = b.post(&format!("goals/G{g}/waves/1/continue"), json!({"who": "Morgan"}));
    let passed = &d["waves"][0]["passed_failed"];
    assert_eq!((passed["by"].as_str(), passed["tasks"].clone()), (Some("Morgan"), json!([format!("T{}", t[0])])), "{d}");
    assert!(passed["at"].is_string());
    assert!(d["stop"].is_null());
    assert!(b.home_entry(g)["stage"].is_null());
}

#[test]
fn passing_a_review_stop_records_no_failed_wave() {
    let b = new_board();
    let (g, t) = planned(&b);
    let from_app: Query = [(api::FROM.to_string(), "app".to_string())].into_iter().collect();
    api::dispatch(&b.app, "POST", &format!("goals/G{g}/waves/1"), &from_app, &json!({"stop_after": true})).unwrap();
    b.set(t[0], "done", false);
    b.set(t[1], "done", false);
    let stop = b.goal(g)["stop"].clone();
    assert_eq!((stop["kind"].as_str(), stop["tasks"].clone()), (Some("review"), json!([])), "{stop}");
    assert_eq!(b.home_entry(g)["stage"]["kind"], "review");
    let d = b.post(&format!("goals/G{g}/waves/1/continue"), json!({"who": "Morgan"}));
    assert!(d["waves"][0]["passed_failed"].is_null());
}

#[test]
fn a_held_wave_reaches_needs_you() {
    let b = new_board();
    let (g, t) = planned(&b);
    b.app.db.x("UPDATE tasks SET status = 'queued', start_job = NULL WHERE goal_id = ?", p![g]).unwrap();
    b.post(&format!("goals/G{g}/waves/1/hold"), json!({}));
    let stop = b.goal(g)["stop"].clone();
    assert_eq!(stop["kind"], "held", "{stop}");
    assert_eq!(stop["line"], "Wave 1 is held until it's continued");
    assert!(stop["at"].is_string(), "when it was held");
    assert!(b.goal(g)["stopped"].is_null(), "the old line stays to review and failed stops");
    let stage = b.home_entry(g)["stage"].clone();
    assert_eq!((stage["kind"].as_str(), stage["at"].clone()), (Some("held"), stop["at"].clone()), "{stage}");
    assert!(stage["holds"]["tasks"].as_array().unwrap().contains(&json!(format!("T{}", t[2]))));

    b.post(&format!("goals/G{g}/waves/1/hold"), json!({"on": false}));
    assert!(b.goal(g)["stop"].is_null());
}

#[test]
fn a_card_says_when_its_terminal_closed() {
    let b = new_board();
    let t = b.task(json!({}));
    b.app.db.x("INSERT INTO sessions(id, name, status) VALUES('s1', 'One', 'working')", p![]).unwrap();
    board::update_task(&b.app, t, fields!["status" => "working", "session_id" => "s1"]).unwrap();
    let c = b.card(t);
    assert_eq!((c["terminal_open"].as_bool(), c["terminal_closed_at"].clone()), (Some(true), Value::Null));

    b.app.db.x("UPDATE sessions SET status = 'gone', gone_at = '2026-11-14T15:04:00Z' WHERE id = 's1'", p![]).unwrap();
    let c = b.card(t);
    assert_eq!((c["terminal_open"].as_bool(), c["terminal_closed_at"].as_str()), (Some(false), Some("2026-11-14T15:04:00Z")));

    let never = b.task(json!({}));
    assert!(b.card(never)["terminal_closed_at"].is_null(), "no terminal yet");
}

#[test]
fn a_wait_links_the_pr_it_waits_on() {
    let b = new_board();
    let first = b.task(json!({"title": "First"}));
    let next = b.task(json!({"title": "Next"}));
    let plain = b.task(json!({"title": "No PR yet"}));
    board::update_task(&b.app, first, fields!["status" => "done", "pr_host" => "github", "pr_repo" => "me/webapp", "pr_num" => 41,
                                              "pr_url" => "https://github.com/me/webapp/pull/41", "pr_state" => "OPEN"]).unwrap();
    b.app.db.x("UPDATE tasks SET waits_for = ? WHERE id = ?", p![json!([first, plain]).to_string(), next]).unwrap();
    let w = b.card(next)["waits_for_state"].clone();
    assert_eq!(w[0]["ref"], format!("T{first}"));
    assert_eq!(w[0]["pr"], json!({"num": 41, "url": "https://github.com/me/webapp/pull/41", "state": "OPEN"}), "{w}");
    assert!(w[1].get("pr").is_none(), "no PR to link: {w}");
}

#[test]
fn a_goal_lists_the_tasks_shared_into_it_with_where_they_came_from() {
    let b = new_board();
    let home = b.new_goal("Auth");
    let other = b.new_goal("Sign-in page");
    let t = b.task(json!({"title": "Add the auth client", "goal_id": home, "also": [format!("G{other}")]}));
    b.task(json!({"title": "Build the form", "goal_id": other}));

    let d = b.goal(other);
    let shared = d["shared_tasks"].as_array().unwrap();
    assert_eq!(shared.len(), 1, "{d}");
    assert_eq!((shared[0]["ref"].clone(), shared[0]["title"].clone()), (json!(format!("T{t}")), json!("Add the auth client")));
    assert_eq!(shared[0]["from"], json!({"id": home, "ref": format!("G{home}"), "name": "Auth"}));
    assert_eq!(b.goal(home)["shared_tasks"], json!([]), "its own goal lists it as its own");
    let listed = b.get("goals");
    let row = listed.as_array().or(listed["goals"].as_array()).unwrap().iter().find(|x| x["id"].as_i64() == Some(other)).cloned().unwrap();
    assert_eq!(row["shared_tasks"][0]["from"]["ref"], format!("G{home}"), "the goals list carries it too");
}

fn issue(b: &Board, title: &str, kind: &str) -> String {
    b.post("backlog", json!({"title": title, "kind": kind, "project": "webapp"}))["ref"].as_str().unwrap().to_string()
}

fn refs(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| x["ref"].as_str().unwrap().to_string()).collect()
}

#[test]
fn backlog_groups_have_true_totals_in_the_order_the_page_shows() {
    let b = new_board();
    let bugs: Vec<String> = (0..3).map(|n| issue(&b, &format!("Bug {n}"), "bug")).collect();
    let gap = issue(&b, "A gap", "gap");
    let more_bugs: Vec<String> = (3..5).map(|n| issue(&b, &format!("Bug {n}"), "bug")).collect();

    // Similar: biggest group first, with every issue in it though the page holds two.
    let v = b.get("backlog/triage?lens=similar&limit=2");
    let groups = v["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{v}");
    assert_eq!((groups[0]["key"].as_str(), groups[0]["total"].as_i64()), (Some("Bugs"), Some(5)));
    assert_eq!(groups[1]["total"], 1);
    assert_eq!(groups[1]["refs"], json!([gap]));
    let all_bugs: Vec<String> = more_bugs.iter().rev().chain(bugs.iter().rev()).cloned().collect();
    assert_eq!(groups[0]["refs"], json!(all_bugs), "newest first in the group");
    // The page comes group by group: the bugs, then the gap, across pages.
    assert_eq!(refs(&v["issues"]), all_bugs[..2].to_vec());
    let next = v["next"].as_str().unwrap().to_string();
    let v2 = b.get(&format!("backlog/triage?lens=similar&limit=2&before={next}"));
    assert_eq!(refs(&v2["issues"]), all_bugs[2..4].to_vec());
    let v3 = b.get(&format!("backlog/triage?lens=similar&limit=2&before={}", v2["next"].as_str().unwrap()));
    assert_eq!(refs(&v3["issues"]), vec![all_bugs[4].clone(), gap.clone()]);
    assert!(v3["next"].is_null());

    // `kinds` keeps those kinds, and the totals follow.
    let v = b.get("backlog/triage?lens=similar&kinds=gap");
    assert_eq!(refs(&v["issues"]), vec![gap.clone()]);
    assert_eq!(v["groups"], json!([{"key": v["groups"][0]["key"], "about": v["groups"][0]["about"], "total": 1, "refs": [gap]}]));

    // No lens: newest first, no groups.
    let v = b.get("backlog/triage?limit=1");
    assert_eq!(refs(&v["issues"]), vec![more_bugs[1].clone()]);
    assert!(v["groups"].is_null());
}

#[test]
fn backlog_filter_words_and_kind_counts() {
    let b = new_board();
    let crash = issue(&b, "Sign-in crashes on reload", "bug");
    issue(&b, "Sidebar flickers", "bug");
    let gap = issue(&b, "No test for sign-in", "gap");
    issue(&b, "Tidy the logger", "clean");

    let v = b.get("backlog/triage?lens=similar");
    assert_eq!(v["kind_counts"], json!({"bug": 2, "gap": 1, "clean": 1}), "{v}");

    // `q` matches the title or the ref, any case, and the counts and groups follow it.
    let v = b.get("backlog/triage?lens=similar&q=SIGN-IN");
    assert_eq!(refs(&v["issues"]).len(), 2, "{v}");
    assert_eq!(v["kind_counts"], json!({"bug": 1, "gap": 1}));
    let totals: i64 = v["groups"].as_array().unwrap().iter().map(|g| g["total"].as_i64().unwrap()).sum();
    assert_eq!(totals, 2);
    // The kind choice narrows the rows but not the counts, so the other kinds' counts stay.
    let v = b.get("backlog/triage?lens=similar&q=sign-in&kinds=gap");
    assert_eq!(refs(&v["issues"]), vec![gap]);
    assert_eq!(v["kind_counts"], json!({"bug": 1, "gap": 1}));
    let v = b.get(&format!("backlog/triage?q={}", crash.to_lowercase()));
    assert_eq!(refs(&v["issues"]), vec![crash]);
}

#[test]
fn backlog_groups_by_priority_rank() {
    let b = new_board();
    let low = issue(&b, "Low", "bug");
    let high = issue(&b, "High", "bug");
    b.post("backlog/bulk", json!({"ids": [high], "action": "priority", "priority": "p1"}));
    let v = b.get("backlog/triage?lens=priority");
    let keys: Vec<&str> = v["groups"].as_array().unwrap().iter().map(|g| g["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["p1", "p2"], "{v}");
    assert_eq!(refs(&v["issues"]), vec![high, low]);
}
