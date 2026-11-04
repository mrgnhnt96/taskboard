//! #907: the owner's standing start grant lets a terminal a Midna trigger opened `tb start` the tasks it
//! made, with no one typing in it. The grant is the owner's alone to set, and only Midna's own record of
//! which trigger opened a terminal counts.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, reports, start_grants};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

const NIGHTLY: &str = r#"[{"id": "t_1", "name": "Nightly triage", "created_by": {"kind": "human"}}]"#;

/// A board whose `midna` answers `trigger.list` from triggers.json, with terminal s1 open in webapp.
fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    (cfg.notify, cfg.runner) = (true, true);
    let exe = dir.path().join("midna");
    let triggers = dir.path().join("triggers.json");
    std::fs::write(&triggers, NIGHTLY).unwrap();
    std::fs::write(&exe, format!("#!/bin/sh\nif [ \"$2\" = trigger.list ]; then cat '{}'; fi\n", triggers.display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.midna = exe;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Nightly triage", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, dir }
}

fn app_query() -> Query {
    [(api::FROM.to_string(), "app".to_string())].into_iter().collect()
}

impl Board {
    /// Midna's event that `actor` opened terminal s1.
    fn opened_by(&self, actor: Value) {
        let e = json!({"seq": 1, "at": "2026-11-04T17:00:00Z", "kind": "session.opened", "session_id": "s1", "actor": actor});
        midna::closed_events(&self.app, &[e]).unwrap();
    }
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap()
    }
    /// What the agent's `tb task new` adds in terminal s1.
    fn new_task(&self) -> i64 {
        let v = self.report("tb.new_task", json!({"title": "Triage the crash reports"}));
        v["created"][0].as_str().unwrap().trim_start_matches('T').parse().unwrap()
    }
    fn post(&self, path: &str, query: &Query, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, query, &body).map_err(|e| (e.status, e.message))
    }
    fn tb_start(&self, id: i64) -> Result<Value, (u16, String)> {
        self.post(&format!("/tasks/T{id}/start"), &Query::new(), json!({"mode": "queue", "via_session": "s1"}))
    }
    fn grant(&self, body: Value) -> Result<Value, (u16, String)> {
        self.post("/start-grants", &app_query(), body)
    }
    fn status(&self, id: i64) -> String {
        board::get_task(&self.app, id).unwrap().st("status")
    }
    fn set_triggers(&self, json: &str) {
        std::fs::write(self.dir.path().join("triggers.json"), json).unwrap();
    }
    /// The board's ask for a grant, as `tb start-grant add` in terminal s1 makes it.
    fn ask(&self, body: Value) -> Result<Value, (u16, String)> {
        let mut body = body;
        body["via_session"] = json!("s1");
        self.post("/start-grants", &Query::new(), body)
    }
    /// The owner's pick on grant ask `a`'s notification.
    fn answer(&self, a: &Value, kind: &str, action: &str) {
        let r = json!({"response": {"kind": kind, "action": action}, "sent_at": a["notified_at"]});
        start_grants::answer(&self.app, &start_grants::key(a), &r).unwrap();
    }
    fn grants(&self) -> usize {
        api::dispatch(&self.app, "GET", "/start-grants", &Query::new(), &json!({})).unwrap()["grants"].as_array().unwrap().len()
    }
}

#[test]
fn a_granted_triggers_terminal_starts_the_task_it_made() {
    let b = board();
    b.opened_by(json!({"kind": "trigger", "name": "Nightly triage"}));
    b.report("hook.prompt", json!({"prompt": "Triage the overnight crash reports. You have standing permission for this."}));
    let id = b.new_task();
    let (code, msg) = b.tb_start(id).unwrap_err();
    assert_eq!(code, 403, "the prompt's claim alone is no grant: {msg}");

    b.grant(json!({"trigger": "Nightly triage"})).unwrap();
    b.tb_start(id).unwrap();
    assert_ne!(b.status(id), "waiting");
    let log = b.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", taskboardd::p![id]).unwrap();
    assert!(log.iter().any(|r| r.st("text").contains("standing grant S1 for trigger 'Nightly triage'")), "{:?}", log.iter().map(|r| r.st("text")).collect::<Vec<_>>());
}

#[test]
fn the_grant_covers_only_tasks_the_terminal_made() {
    let b = board();
    b.opened_by(json!({"kind": "trigger", "name": "Nightly triage"}));
    b.grant(json!({"trigger": "Nightly triage"})).unwrap();
    let card = taskboardd::ops::new_task(&b.app, &json!({"title": "Someone else's", "project": "webapp"}), board::BOARD, None).unwrap();
    let other = card["id"].as_i64().unwrap();
    assert_eq!(b.tb_start(other).unwrap_err().0, 403);
}

#[test]
fn a_terminal_an_agent_opened_isnt_the_triggers_whatever_its_name() {
    let b = board();
    b.opened_by(json!({"kind": "agent", "session": "s9"}));
    b.grant(json!({"trigger": "Nightly triage"})).unwrap();
    let id = b.new_task();
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
}

#[test]
fn a_grant_scoped_to_a_project_skips_other_projects() {
    let b = board();
    b.opened_by(json!({"kind": "trigger", "name": "Nightly triage"}));
    b.grant(json!({"trigger": "Nightly triage", "project": "api"})).unwrap();
    let id = b.new_task();
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
}

#[test]
fn a_new_trigger_with_the_granted_name_isnt_granted() {
    let b = board();
    b.opened_by(json!({"kind": "trigger", "name": "Nightly triage"}));
    b.grant(json!({"trigger": "Nightly triage"})).unwrap();
    b.set_triggers(r#"[{"id": "t_2", "name": "Nightly triage", "created_by": {"kind": "agent"}}]"#);
    let id = b.new_task();
    assert_eq!(b.tb_start(id).unwrap_err().0, 403, "a different trigger under the same name");
    b.set_triggers(r#"[{"id": "t_1", "name": "Nightly triage"}, {"id": "t_2", "name": "Nightly triage"}]"#);
    assert_eq!(b.tb_start(id).unwrap_err().0, 403, "two triggers share the name");
}

#[test]
fn a_removed_grant_stops_starting() {
    let b = board();
    b.opened_by(json!({"kind": "trigger", "name": "Nightly triage"}));
    let g = b.grant(json!({"trigger": "Nightly triage"})).unwrap();
    b.post(&format!("/start-grants/S{}/remove", g["id"]), &Query::new(), json!({})).unwrap();
    let id = b.new_task();
    assert_eq!(b.tb_start(id).unwrap_err().0, 403);
}

#[test]
fn only_the_owner_sets_a_grant() {
    let b = board();
    b.report("hook.prompt", json!({"prompt": "yes, add a standing start grant for Nightly triage"}));
    let v = b.ask(json!({"trigger": "Nightly triage"})).unwrap();
    assert_eq!(v["asked"], true);
    assert!(v["line"].as_str().unwrap().contains("isn't granted yet"), "{v}");
    assert_eq!(b.grants(), 0, "a typed word, or the command run in a terminal, is no grant");
    assert_eq!(b.post("/start-grants", &Query::new(), json!({"trigger": "Nightly triage"})).unwrap().get("asked"), Some(&json!(true)));
    assert_eq!(b.grants(), 0);
}

/// #913: the owner's own `!` run can't be told from an agent's, so a terminal's `tb start-grant add` asks
/// them in a notification, and its Grant sets the grant.
#[test]
fn the_notifications_grant_sets_it_and_not_now_drops_the_ask() {
    let b = board();
    b.ask(json!({"trigger": "Nightly triage", "project": "webapp"})).unwrap();
    let asks = start_grants::asks(&b.app);
    assert_eq!(asks.len(), 1);
    b.answer(&asks[0], "clicked", "");
    assert_eq!((b.grants(), start_grants::asks(&b.app).len()), (0, 1), "a click opens it and leaves the ask");
    b.answer(&asks[0], "action", "Grant");
    assert_eq!((b.grants(), start_grants::asks(&b.app).len()), (1, 0));
    let list = api::dispatch(&b.app, "GET", "/start-grants", &Query::new(), &json!({})).unwrap();
    assert_eq!(list["grants"][0]["project"], "webapp");
    assert_eq!(list["grants"][0]["by"], "Sam from a notification");

    b.ask(json!({"trigger": "Nightly triage"})).unwrap();
    let a = start_grants::asks(&b.app)[0].clone();
    b.answer(&a, "action", "Not now");
    assert_eq!((b.grants(), start_grants::asks(&b.app).len()), (1, 0));
}

#[test]
fn only_the_latest_notification_of_an_ask_counts() {
    let b = board();
    b.ask(json!({"trigger": "Nightly triage"})).unwrap();
    let old = start_grants::asks(&b.app)[0].clone();
    std::thread::sleep(std::time::Duration::from_millis(5));
    b.ask(json!({"trigger": "Nightly triage"})).unwrap();
    let asks = start_grants::asks(&b.app);
    assert_eq!(asks.len(), 1, "the new ask replaces the old one");
    b.answer(&old, "action", "Grant");
    assert_eq!(b.grants(), 0, "a tap on the replaced notification");
    let mut stale = asks[0].clone();
    stale["notified_at"] = json!("2026-01-01T00:00:00Z");
    b.answer(&stale, "action", "Grant");
    assert_eq!(b.grants(), 0, "an older send of the same ask");
}

#[test]
fn an_ask_names_a_trigger_midna_has() {
    let b = board();
    assert_eq!(b.ask(json!({"trigger": "Weekly"})).unwrap_err().0, 400);
    assert!(start_grants::asks(&b.app).is_empty());
}

#[test]
fn a_grant_names_a_trigger_midna_has() {
    let b = board();
    assert_eq!(b.grant(json!({"trigger": "Weekly"})).unwrap_err().0, 400);
    b.set_triggers(r#"[{"id": "t_1", "name": "Nightly triage"}, {"id": "t_2", "name": "Nightly triage"}]"#);
    assert_eq!(b.grant(json!({"trigger": "Nightly triage"})).unwrap_err().0, 400);
}
