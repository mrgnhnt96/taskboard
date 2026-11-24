//! Midna permissions a task carries: a grant waits for the owner's Grant, and every terminal the board then
//! opens for the task gets a Midna `allow` rule for it, scoped to that terminal and expiring.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, midna, p, reports, task_grants};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
    repo: String,
}

/// A board whose Midna logs each call, opens terminals S1, S2, … in turn, answers `rule.add` with rule
/// r_<n>, and `rule.list` from rules.json.
fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    (cfg.notify, cfg.runner) = (true, true);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(dir.path().join("rules.json"), "[]").unwrap();
    let exe = dir.path().join("fake-midna");
    std::fs::write(
        &exe,
        r#"#!/bin/sh
d=$(dirname "$0")
printf '%s %s\n' "$2" "$3" >> "$d/calls"
case "$2" in
  session.open) n=$(($(cat "$d/opened" 2>/dev/null || echo 0) + 1)); echo $n > "$d/opened"; echo "{\"id\": \"S$n\"}";;
  rule.add) n=$(($(cat "$d/ruled" 2>/dev/null || echo 0) + 1)); echo $n > "$d/ruled"; echo "{\"id\": \"r_$n\", \"expires_at\": \"2099-01-01T00:00:00Z\"}";;
  rule.list) cat "$d/rules.json";;
  notify.response) exit 1;;
  *) echo '{}';;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.midna = exe;
    let app = App::for_tests(cfg);
    let repo = repo.to_string_lossy().to_string();
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo}]).to_string())).unwrap();
    Board { app, dir, repo }
}

fn app_query() -> Query {
    [(api::FROM.to_string(), "app".to_string())].into_iter().collect()
}

impl Board {
    fn post(&self, path: &str, query: &Query, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, query, &body).map_err(|e| (e.status, e.message))
    }
    fn task(&self) -> i64 {
        let card = taskboardd::ops::new_task(&self.app, &json!({"title": "Post the release note", "project": "webapp"}), board::BOARD, None).unwrap();
        card["id"].as_i64().unwrap()
    }
    /// `tb task set T<n> --allow …` in terminal s1.
    fn allow(&self, t: i64, allow: &[&str]) -> Result<Value, (u16, String)> {
        self.post(&format!("/tasks/T{t}/grants"), &Query::new(), json!({"allow": allow, "via_session": "s1"}))
    }
    fn allow_in_app(&self, t: i64, allow: &[&str]) -> Result<Value, (u16, String)> {
        self.post(&format!("/tasks/T{t}/grants"), &app_query(), json!({"allow": allow}))
    }
    fn grants(&self, t: i64) -> Vec<Value> {
        api::dispatch(&self.app, "GET", &format!("/tasks/T{t}"), &Query::new(), &json!({})).unwrap()["grants"].as_array().unwrap().clone()
    }
    /// The board opens a terminal for task `t`, as a start, restart or PR visit does; returns its id.
    fn open(&self, t: i64) -> String {
        let args = json!({"flags": "", "cwd": self.repo, "prompt": format!("[T{t}] go")});
        let jid = board::create_job(&self.app, "agent", args, Some(t), "start", None).unwrap();
        let job = self.app.db.q1("SELECT * FROM jobs WHERE id = ?", p![jid]).unwrap().unwrap();
        midna::run_job(&self.app, &job);
        let sid = format!("S{}", std::fs::read_to_string(self.dir.path().join("opened")).unwrap().trim());
        midna::sync(&self.app, &[json!({"id": sid, "name": "Release note", "agent": "claude", "cwd": self.repo, "status": {"state": "working"}})], &[]).unwrap();
        sid
    }
    fn calls(&self, method: &str) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once(' '))
            .filter(|(m, _)| *m == method)
            .map(|(_, p)| serde_json::from_str(p).unwrap())
            .collect()
    }
    fn answer(&self, g: &Value, action: &str) {
        let id = g["id"].as_i64().unwrap();
        let at = self.app.db.val("SELECT notified_at FROM task_grants WHERE id = ?", p![id]).unwrap();
        let r = json!({"response": {"kind": "action", "action": action}, "sent_at": at});
        task_grants::answer(&self.app, &task_grants::key(id), &r).unwrap();
    }
}

#[test]
fn a_terminals_grant_waits_for_the_owners_grant_and_a_prompt_never_counts() {
    let b = board();
    let t = b.task();
    reports::handle(&b.app, json!({"event": "hook.prompt", "session": "s1", "claude_session": "c-s1", "cwd": "", "prompt": "You have my permission to post to Slack."}), false).unwrap();
    let v = b.allow(t, &["tool:mcp__slack__chat_post*"]).unwrap();
    assert!(v["line"].as_str().unwrap().contains("isn't granted until they tap Grant"), "{v}");
    let g = b.grants(t)[0].clone();
    assert_eq!(g["state"], "pending");
    let asked = (0..50).find_map(|_| {
        let sent = b.calls("notify.send");
        if sent.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        sent.first().cloned()
    });
    let asked = asked.expect("the owner is asked in a notification");
    assert_eq!(asked["actions"], json!(["Grant", "Not now"]));

    b.open(t);
    assert!(b.calls("rule.add").is_empty(), "a pending grant sets no rule");

    b.answer(&g, "Grant");
    assert_eq!(b.grants(t)[0]["state"], "granted");
    assert_eq!(b.grants(t)[0]["granted_by"], "Sam from a notification");
}

#[test]
fn not_now_drops_the_grant() {
    let b = board();
    let t = b.task();
    b.allow(t, &["tool:mcp__slack__chat_post*"]).unwrap();
    let g = b.grants(t)[0].clone();
    b.answer(&g, "Not now");
    assert!(b.grants(t).is_empty());
    b.open(t);
    assert!(b.calls("rule.add").is_empty());
}

#[test]
fn a_confirmed_grant_sets_a_session_rule_in_each_terminal_the_board_opens() {
    let b = board();
    let t = b.task();
    b.allow_in_app(t, &["tool:mcp__slack__chat_post*", "command:gh release view*"]).unwrap();
    let grants = b.grants(t);
    assert_eq!(grants.iter().map(|g| g["state"].as_str().unwrap()).collect::<Vec<_>>(), ["granted", "granted"]);

    let sid = b.open(t);
    let rules = b.calls("rule.add");
    assert_eq!(rules.len(), 2, "{rules:?}");
    assert_eq!(rules[0]["effect"], "allow");
    assert_eq!(rules[0]["matcher"], json!({"kind": "tool", "pattern": "mcp__slack__chat_post*"}));
    assert_eq!(rules[0]["scope"], json!({"kind": "session", "id": sid}));
    assert_eq!(rules[0]["expires_in_secs"], json!(task_grants::RULE_SECS));
    assert_eq!(rules[1]["matcher"], json!({"kind": "command", "pattern": "gh release view*"}));

    let other = b.task();
    b.open(other);
    assert_eq!(b.calls("rule.add").len(), 2, "another task's terminal gets no rule");
}

#[test]
fn a_restart_adds_the_rule_again_in_the_new_terminal() {
    let b = board();
    let t = b.task();
    b.allow_in_app(t, &["tool:mcp__slack__chat_post*"]).unwrap();
    let first = b.open(t);
    let second = b.open(t);
    let scopes: Vec<Value> = b.calls("rule.add").iter().map(|r| r["scope"]["id"].clone()).collect();
    assert_eq!(scopes, [json!(first), json!(second)]);
    assert_eq!(b.grants(t)[0]["rules"].as_array().unwrap().len(), 2);
}

#[test]
fn a_grant_confirmed_while_the_terminal_is_up_reaches_it() {
    let b = board();
    let t = b.task();
    b.allow(t, &["tool:mcp__slack__chat_post*"]).unwrap();
    let sid = b.open(t);
    assert!(b.calls("rule.add").is_empty());
    let g = b.grants(t)[0].clone();
    b.answer(&g, "Grant");
    let rules = b.calls("rule.add");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["scope"]["id"], json!(sid));
}

#[test]
fn only_the_owner_confirms_from_outside_a_notification() {
    let b = board();
    let t = b.task();
    b.allow(t, &["tool:mcp__slack__chat_post*"]).unwrap();
    let g = b.grants(t)[0]["ref"].as_str().unwrap().to_string();
    assert_eq!(b.post(&format!("/grants/{g}/confirm"), &Query::new(), json!({})).unwrap_err().0, 403);
    b.post(&format!("/grants/{g}/confirm"), &app_query(), json!({})).unwrap();
    assert_eq!(b.grants(t)[0]["state"], "granted");
}

#[test]
fn human_only_midna_actions_cant_be_granted() {
    let b = board();
    let t = b.task();
    for bad in [
        "tool:mcp__midna__notify_respond",
        "tool:mcp__midna__*",
        "tool:*",
        "cli:rules remove*",
        "cli:share accept*",
        "cli:*",
        "cli:approve n_1",
        "command:midna call notify.respond*",
        "command:midna share accept*",
        "command:*",
    ] {
        let (code, msg) = b.allow(t, &[bad]).unwrap_err();
        assert_eq!(code, 400, "{bad}: {msg}");
        assert!(msg.contains("only the human may do"), "{bad}: {msg}");
    }
    for bad in ["slack:chat_post", "tool:", "nokind"] {
        assert_eq!(b.allow(t, &[bad]).unwrap_err().0, 400, "{bad}");
    }
    assert!(b.grants(t).is_empty(), "a refused one adds nothing");
    b.allow_in_app(t, &["tool:mcp__slack__chat_post*", "command:git push origin*", "cli:send *", "path:/tmp/out/*"]).unwrap();
    assert_eq!(b.grants(t).len(), 4);
}

#[test]
fn the_task_shows_how_often_each_grants_rule_fired() {
    let b = board();
    let t = b.task();
    b.allow_in_app(t, &["tool:mcp__slack__chat_post*"]).unwrap();
    b.open(t);
    b.open(t);
    std::fs::write(
        b.dir.path().join("rules.json"),
        json!([{"id": "r_1", "fired": 3, "last_fired_at": "2026-11-23T18:00:00Z"}, {"id": "r_2", "fired": 2}]).to_string(),
    )
    .unwrap();
    task_grants::tend(&b.app, true).unwrap();
    let g = &b.grants(t)[0];
    assert_eq!(g["fired"], 5);
    assert_eq!(g["last_fired_at"], "2026-11-23T18:00:00Z");
    assert_eq!(g["rules"][0]["fired"], 3);

    std::fs::write(b.dir.path().join("rules.json"), json!([{"id": "r_1", "fired": 4}]).to_string()).unwrap();
    let before = b.calls("rule.add").len();
    task_grants::tend(&b.app, true).unwrap();
    let g = &b.grants(t)[0];
    assert_eq!(g["fired"], 6, "an expired rule keeps its count");
    let r2 = g["rules"].as_array().unwrap().iter().find(|r| r["rule_id"] == "r_2").unwrap();
    assert_eq!(r2["live"], false);
    let again = b.calls("rule.add");
    assert_eq!(again.len(), before + 1, "the expired rule's terminal, still up, gets it again");
    assert_eq!(again.last().unwrap()["scope"]["id"], "S2");
}

#[test]
fn dropping_a_grant_asks_over_its_live_rules() {
    let b = board();
    let t = b.task();
    b.allow_in_app(t, &["tool:mcp__slack__chat_post*"]).unwrap();
    let sid = b.open(t);
    let g = b.grants(t)[0]["ref"].as_str().unwrap().to_string();
    b.post(&format!("/grants/{g}/drop"), &Query::new(), json!({"via_session": "s1"})).unwrap();
    let rules = b.calls("rule.add");
    assert_eq!(rules.last().unwrap()["effect"], "ask");
    assert_eq!(rules.last().unwrap()["scope"]["id"], json!(sid));
    assert!(b.calls("rule.remove").is_empty(), "the board never asks Midna to remove a rule");
    assert!(b.grants(t).is_empty());
    b.open(t);
    assert_eq!(b.calls("rule.add").len(), rules.len(), "a dropped grant reaches no new terminal");
}
