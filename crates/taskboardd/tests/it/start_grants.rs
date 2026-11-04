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
use taskboardd::{board, midna, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

const NIGHTLY: &str = r#"[{"id": "t_1", "name": "Nightly triage", "created_by": {"kind": "human"}}]"#;

/// A board whose `midna` answers `trigger.list` from triggers.json, with terminal s1 open in webapp.
fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
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
    /// The owner running `cmd` with Claude Code's `!` prefix in terminal s1.
    fn owner_runs(&self, cmd: &str) {
        self.report("hook.prompt", json!({"prompt": "hi"}));
        let dir = self.app.cfg.claude_projects.join("-webapp");
        std::fs::create_dir_all(&dir).unwrap();
        let lines = [json!({"type": "user", "message": {"content": "hi"}}), json!({"type": "user", "message": {"content": format!("<bash-input>{cmd}</bash-input>")}})];
        std::fs::write(dir.join("c-s1.jsonl"), lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
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
    let agent = |prompt: &str| {
        b.report("hook.prompt", json!({"prompt": prompt}));
        b.post("/start-grants", &Query::new(), json!({"trigger": "Nightly triage", "via_session": "s1"}))
    };
    let (code, msg) = agent("yes, add a standing start grant for Nightly triage").unwrap_err();
    assert_eq!(code, 403, "a typed word is no grant: {msg}");
    assert_eq!(b.post("/start-grants", &Query::new(), json!({"trigger": "Nightly triage"})).unwrap_err().0, 403);

    b.owner_runs("tb start-grant add --trigger \"Nightly triage\"");
    b.post("/start-grants", &Query::new(), json!({"trigger": "Nightly triage", "via_session": "s1"})).unwrap();
    let list = api::dispatch(&b.app, "GET", "/start-grants", &Query::new(), &json!({})).unwrap();
    assert_eq!(list["grants"].as_array().unwrap().len(), 1);
}

#[test]
fn a_grant_names_a_trigger_midna_has() {
    let b = board();
    assert_eq!(b.grant(json!({"trigger": "Weekly"})).unwrap_err().0, 400);
    b.set_triggers(r#"[{"id": "t_1", "name": "Nightly triage"}, {"id": "t_2", "name": "Nightly triage"}]"#);
    assert_eq!(b.grant(json!({"trigger": "Nightly triage"})).unwrap_err().0, 400);
}
