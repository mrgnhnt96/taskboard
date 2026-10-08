//! QA comments (`docs/parity/oct8-changes.md` #10a, #13a): off until switched on, read from Jira
//! comments on board tickets, and turned into a follow-up task, a flag for the owner, or nothing.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, qa};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn board_with(jira: bool) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    if jira {
        cfg.jira.site = "acme.atlassian.net".into();
        cfg.jira.project = "PROJ".into();
    }
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, _dir: dir }
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
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    /// A goal with a done task on PROJ-7 (its PR #12), the ticket QA comments land on.
    fn shipped(&self) -> (i64, i64) {
        let g = self.post("/goals", json!({"name": "Login", "project": "webapp"}))["id"].as_i64().unwrap();
        let t = self.post("/tasks", json!({"title": "Add login", "project": "webapp", "goal_id": g}))["id"].as_i64().unwrap();
        board::update_task(&self.app, t, taskboardd::fields!["status" => "done", "jira_key" => "PROJ-7", "pr_num" => 12]).unwrap();
        (g, t)
    }
    fn comment(&self, id: &str, text: &str) -> i64 {
        let r = self.post("/jira/comment", json!({"key": "PROJ-7", "comment_id": id, "author": "Sam QA", "text": text}));
        assert_eq!(r["new"], true, "{r}");
        r["comment"].as_str().unwrap()[1..].parse().unwrap()
    }
    fn decide(&self, id: i64, out: Value) {
        self.app.db.tx(|| qa::decide(&self.app, &qa::get(&self.app, id)?, &out)).unwrap();
    }
}

#[test]
fn it_is_off_until_switched_on_and_needs_jira() {
    let b = board_with(false);
    assert_eq!(b.get("/qa")["on"], false);
    let (code, msg) = b.post_err("/qa", json!({"on": true}));
    assert_eq!(code, 409);
    assert!(msg.contains("Jira"), "{msg}");

    let b = board_with(true);
    b.shipped();
    let r = b.post("/jira/comment", json!({"key": "PROJ-7", "comment_id": "100", "text": "Broken"}));
    assert_eq!(r["ignored"], "QA comments are off");
    let s = b.post("/qa", json!({"on": true}));
    assert_eq!(s["on"], true);
    assert!(s["since"].is_string(), "comments from before it was on are never read");
    assert_eq!(b.post("/jira/comment", json!({"key": "PROJ-9", "comment_id": "101"}))["ignored"], "PROJ-9 isn't on the board");
}

#[test]
fn a_clear_direction_becomes_a_follow_up_task_and_more_comments_add_to_it() {
    let b = board_with(true);
    b.post("/qa", json!({"on": true}));
    let (g, src) = b.shipped();
    let q1 = b.comment("200", "The button overlaps the logo on small phones.");
    assert_eq!(qa::get(&b.app, q1).unwrap().i0("tries"), 1, "reading it failed here (no claude) and it waits to try again");
    b.decide(q1, json!({"verdict": "task", "pr": true, "title": "Fix the overlap", "ask": "Move the button below the logo on small screens."}));

    let c = qa::get(&b.app, q1).unwrap();
    let fu = board::get_task(&b.app, c.i0("task_id")).unwrap();
    assert_eq!(fu.s("title"), Some("QA on PROJ-7: Fix the overlap"));
    assert_eq!(fu.s("priority"), Some("high"));
    assert_eq!(fu.i("goal_id"), Some(g));
    assert!(fu.st("detail").contains("Move the button below the logo"));
    assert!(fu.st("detail").contains(&format!("T{src}")));
    let detail = b.get(&format!("/tasks/T{}", fu.id()));
    assert_eq!(detail["origin"]["from"], "Jira comment on PROJ-7");
    assert_eq!(detail["origin"]["by"], "Sam QA");
    assert!(detail["origin"]["url"].as_str().unwrap().ends_with("focusedCommentId=200"));

    let q2 = b.comment("201", "Also the spinner never stops.");
    b.decide(q2, json!({"verdict": "task", "title": "Stop the spinner", "ask": "Stop the spinner after login."}));
    assert_eq!(qa::get(&b.app, q2).unwrap().i("task_id"), Some(fu.id()), "an open follow-up takes the next comment");
    assert!(board::get_task(&b.app, fu.id()).unwrap().st("detail").contains("Stop the spinner after login."));

    let tab = b.get(&format!("/goals/G{g}"))["qa"].clone();
    assert_eq!(tab.as_array().unwrap().len(), 2);
}

#[test]
fn a_question_flags_the_owner_until_they_say_what_to_do() {
    let b = board_with(true);
    b.post("/qa", json!({"on": true}));
    let (g, _) = b.shipped();
    let pass = b.comment("300", "Passed on iOS.");
    b.decide(pass, json!({"verdict": "none", "title": "", "ask": ""}));
    let q = b.comment("301", "Should the old login stay for a release?");
    b.decide(q, json!({"verdict": "flag", "title": "Keep old login?", "ask": "Should the old login stay for a release?"}));

    let alerts = dispatch::alerts(&b.app);
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0]["text"].as_str().unwrap().starts_with(&format!("Q{q} · Sam QA on PROJ-7: Should the old login")));
    let tab = b.get(&format!("/goals/G{g}"))["qa"].clone();
    assert_eq!(tab.as_array().unwrap().len(), 1, "comments that need nothing stay off the tab");
    assert_eq!(tab[0]["waiting"], true);
    assert_eq!(b.get("/qa-comments?waiting=1")["comments"].as_array().unwrap().len(), 1);

    let r = b.post(&format!("/qa-comments/Q{q}"), json!({"action": "ignore", "who": "Term s1"}));
    assert_eq!(r["waiting"], false);
    assert!(dispatch::alerts(&b.app).is_empty());
    let (code, _) = b.post_err(&format!("/qa-comments/Q{q}"), json!({"action": "task"}));
    assert_eq!(code, 409, "it was handled already");

    let q3 = b.comment("302", "Can we log the failures?");
    b.decide(q3, json!({"verdict": "flag", "title": "Log failures", "ask": "Can we log the failures?"}));
    let r = b.post(&format!("/qa-comments/Q{q3}"), json!({"action": "task", "note": "Yes, log them at warn.", "pr": false}));
    let t = board::get_task(&b.app, r["started"].as_str().unwrap()[1..].parse().unwrap()).unwrap();
    assert!(t.st("detail").contains("says: Yes, log them at warn."));
    assert!(t.st("detail").contains("No PR") || t.st("detail").contains("report back"));
}

#[test]
fn switching_it_off_hides_the_tab_and_its_alerts() {
    let b = board_with(true);
    b.post("/qa", json!({"on": true}));
    let (g, _) = b.shipped();
    let q = b.comment("400", "Is this right?");
    b.decide(q, json!({"verdict": "flag", "title": "Right?", "ask": "Is this right?"}));
    assert_eq!(dispatch::alerts(&b.app).len(), 1);
    b.post("/qa", json!({"on": false}));
    assert!(dispatch::alerts(&b.app).is_empty());
    assert_eq!(b.get(&format!("/goals/G{g}"))["qa"], json!([]));
    assert_eq!(b.get("/qa-comments")["on"], false);
}

#[test]
fn jira_rich_text_reads_as_plain_lines() {
    let body = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Hi "}, {"type": "mention", "attrs": {"text": "@Sam"}}, {"type": "text", "text": ","}]},
        {"type": "bulletList", "content": [
            {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "logo overlaps"}]}]},
            {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "see "}, {"type": "inlineCard", "attrs": {"url": "https://x/1"}}]}]}]}]});
    assert_eq!(taskboardd::jira::adf_text(&body), "Hi @Sam,\nlogo overlaps\nsee https://x/1");
    assert_eq!(taskboardd::jira::adf_text(&json!("plain")), "plain");
}
