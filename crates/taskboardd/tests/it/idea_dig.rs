//! Digging into an idea: a read-only headless Claude in the project's checkout reports whether the
//! idea fits, kept as a round with the commit it read; failures and lost digs stay on the round.

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{fields, p};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let claude = dir.path().join("fake-claude");
    std::fs::write(&claude, "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" >> \"$d/all-args.txt\"\ncat \"$d/answer.json\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = claude.to_string_lossy().to_string();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, dir }
}

fn report() -> Value {
    json!({"verdict": "partly", "why": "The store is there; the runner isn't.",
        "builds_on": [{"what": "The tidy's headless call", "files": ["src/tidy.rs"]}],
        "ways_to_extend": ["Dig on a branch"], "risks": [{"kind": "privacy", "risk": "Sends code to Claude"}],
        "pitfalls": ["Long runs"], "open_questions": ["Web by default?"], "related": ["G999"]})
}

impl Board {
    fn repo(&self) -> std::path::PathBuf {
        self.dir.path().join("webapp")
    }
    fn try_post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        self.try_post(path, body).unwrap_or_else(|e| panic!("POST {path}: {e:?}"))
    }
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn idea(&self) -> String {
        let v = self.post("/ideas", json!({"title": "Offline mode", "detail": "Queue writes while offline.", "project": "webapp"}));
        v["ref"].as_str().unwrap().to_string()
    }
    fn answer(&self, v: Value) {
        std::fs::write(self.dir.path().join("answer.json"), json!({"structured_output": v}).to_string()).unwrap();
    }
    /// Claude's arguments since the last call, cleared.
    fn calls(&self) -> String {
        let f = self.dir.path().join("all-args.txt");
        let s = std::fs::read_to_string(&f).unwrap_or_default();
        let _ = std::fs::remove_file(f);
        s
    }
}

#[test]
fn a_dig_keeps_claudes_report_with_the_commit_it_read() {
    let b = board();
    let i = b.idea();
    b.post(&format!("/ideas/{i}/note"), json!({"text": "Only for the inbox at first."}));
    b.answer(report());
    let v = b.post(&format!("/ideas/{i}/dig"), json!({"focus": "conflicts on sync"}));

    let args = b.calls();
    assert!(args.contains("--tools\nRead,Grep,Glob\n"), "read-only tools: {args}");
    assert!(!args.contains("WebSearch") && !args.contains("Bash") && !args.contains("Edit"), "{args}");
    assert!(args.contains("Validate the idea; don't plan it."), "{args}");
    assert!(args.contains("Only for the inbox at first.") && args.contains("conflicts on sync"), "notes and focus: {args}");

    assert_eq!(v["state"], "reported");
    let r = &v["rounds"][0];
    assert_eq!(r["focus"], "conflicts on sync");
    assert_eq!(r["sha"], json!(git(&b.repo(), &["rev-parse", "HEAD"])));
    assert_eq!(r["web"], false);
    assert_eq!(r["error"], Value::Null);
    assert!(r["finished_at"].is_string());
    assert_eq!(r["report"]["verdict"], "partly");
    assert_eq!(r["report"]["builds_on"][0]["files"], json!(["src/tidy.rs"]));
    assert_eq!(r["report"]["related"], json!([]), "a ref the prompt didn't list is left out");
    assert_eq!((r["behind"].clone(), r["stale"].clone()), (json!(0), json!(false)));

    b.answer(report());
    let v = b.post(&format!("/ideas/{i}/dig"), json!({"web": true}));
    let args = b.calls();
    assert!(args.contains("Read,Grep,Glob,WebSearch,WebFetch"), "the web only when asked: {args}");
    assert!(args.contains("Earlier rounds") && args.contains("Web by default?"), "earlier rounds are in the prompt: {args}");
    assert_eq!(v["rounds"].as_array().unwrap().len(), 2);
    assert_eq!((v["rounds"][0]["focus"].clone(), v["rounds"][0]["web"].clone()), (json!(""), json!(true)));
}

#[test]
fn a_failed_dig_keeps_its_error_on_the_round() {
    let b = board();
    let i = b.idea();
    std::fs::write(b.dir.path().join("answer.json"), "not json").unwrap();
    let v = b.post(&format!("/ideas/{i}/dig"), json!({}));
    assert_eq!(v["state"], "new", "no report yet, so it's new again");
    let r = &v["rounds"][0];
    assert!(r["error"].as_str().is_some_and(|e| !e.is_empty()), "{r}");
    assert!(r["finished_at"].is_string());
    assert_eq!(r["report"], Value::Null);
}

#[test]
fn one_dig_at_a_time_and_none_on_an_idea_put_aside() {
    let b = board();
    let i = b.idea();
    let id: i64 = i[1..].parse().unwrap();
    b.app.shared.lock().digging.insert(id);
    let (code, msg) = b.try_post(&format!("/ideas/{i}/dig"), json!({})).unwrap_err();
    assert_eq!(code, 409, "{msg}");
    assert!(msg.contains("being dug into already"), "{msg}");
    b.app.shared.lock().digging.remove(&id);

    b.post(&format!("/ideas/{i}/shelve"), json!({}));
    let (code, msg) = b.try_post(&format!("/ideas/{i}/dig"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("reopen it first"), "{msg}");
    assert_eq!(b.calls(), "", "Claude wasn't asked");
}

#[test]
fn a_dig_the_board_lost_is_closed_as_failed() {
    let b = board();
    let i = b.idea();
    let id: i64 = i[1..].parse().unwrap();
    b.app.db.insert("idea_rounds", fields!["idea_id" => id, "focus" => "", "web" => 0, "started_at" => "2026-10-19T12:00:00Z"]).unwrap();
    b.app.db.x("UPDATE ideas SET state = 'digging' WHERE id = ?", p![id]).unwrap();

    let v = b.get(&format!("/ideas/{i}"));
    assert_eq!(v["state"], "new");
    assert!(v["rounds"][0]["error"].as_str().unwrap().contains("board stopped"), "{}", v["rounds"][0]);
    assert!(v["rounds"][0]["finished_at"].is_string());

    b.answer(report());
    b.post(&format!("/ideas/{i}/dig"), json!({}));
    b.app.db.insert("idea_rounds", fields!["idea_id" => id, "focus" => "", "web" => 0, "started_at" => "2026-10-19T12:00:00Z"]).unwrap();
    b.app.db.x("UPDATE ideas SET state = 'digging' WHERE id = ?", p![id]).unwrap();
    let v = b.get(&format!("/ideas/{i}"));
    assert_eq!(v["state"], "reported", "a round with a report brings it back reported");
    assert_eq!(v["rounds"][1]["report"]["verdict"], "partly", "the earlier report is kept");
}

#[test]
fn a_round_goes_stale_once_the_checkout_moves_far() {
    let b = board();
    let i = b.idea();
    b.answer(report());
    b.post(&format!("/ideas/{i}/dig"), json!({}));
    for n in 0..50 {
        git(&b.repo(), &["commit", "-q", "--allow-empty", "-m", &format!("c{n}")]);
    }
    let r = &b.get(&format!("/ideas/{i}"))["rounds"][0];
    assert_eq!((r["behind"].clone(), r["stale"].clone()), (json!(50), json!(true)));
}
