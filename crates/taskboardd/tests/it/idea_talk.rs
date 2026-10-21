//! Talking an idea over: a Claude terminal in the project's checkout, primed with the idea, its notes
//! and its reports, asked about one open question when the owner picks one.

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

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, dir }
}

impl Board {
    fn try_post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        self.try_post(path, body).unwrap_or_else(|e| panic!("POST {path}: {e:?}"))
    }
    /// The job's purpose and arguments.
    fn job(&self, v: &Value) -> (String, Value) {
        let jid: i64 = v["job"].as_str().unwrap()[1..].parse().unwrap();
        let j = self.app.db.q1("SELECT purpose, args FROM jobs WHERE id = ?", p![jid]).unwrap().unwrap();
        (j["purpose"].as_str().unwrap().to_string(), serde_json::from_str(j["args"].as_str().unwrap()).unwrap())
    }
}

#[test]
fn talking_an_idea_over_opens_claude_in_its_checkout_with_all_of_it() {
    let b = board();
    let i = b.post("/ideas", json!({"title": "Offline mode", "detail": "Queue writes while offline.", "project": "webapp"}))["ref"]
        .as_str().unwrap().to_string();
    b.post(&format!("/ideas/{i}/note"), json!({"text": "Only for the inbox at first."}));
    let id: i64 = i[1..].parse().unwrap();
    let report = json!({"verdict": "partly", "why": "The store is there.", "blocks": [
        {"type": "gist", "text": "The store is there; the sync isn't."},
        {"type": "question", "id": "conflicts", "text": "Last write wins?", "choices": ["Yes", "Ask"]}]});
    let old = json!({"verdict": "fits", "why": "It fits.", "open_questions": ["Which store?"]});
    for rep in [old, report] {
        b.app.db.insert("idea_rounds", fields!["idea_id" => id, "focus" => "", "web" => 0, "report" => rep.to_string()]).unwrap();
    }

    let (code, msg) = b.try_post(&format!("/ideas/{i}/talk"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("Midna isn't running"), "{msg}");
    b.app.shared.lock().midna_seen_at = Some(taskboardd::util::now_iso());

    let v = b.post(&format!("/ideas/{i}/talk"), json!({}));
    assert_eq!(v["ok"], true);
    let (purpose, a) = b.job(&v);
    assert_eq!(purpose, "idea_talk");
    assert_eq!(a["cwd"], b.dir.path().join("webapp").to_string_lossy().as_ref());
    assert_eq!((&a["title"], &a["prompt"], &a["draft"], &a["focus"]), (&json!("Talk: Offline mode"), &json!(""), &json!(""), &json!(true)),
               "Claude waits for the owner, in a terminal brought to the front");
    let c = a["context"].as_str().unwrap();
    assert!(c.contains("Queue writes while offline.") && c.contains("Only for the inbox at first."), "detail and notes: {c}");
    assert!(c.contains("Last write wins? (choices: Yes / Ask)"), "the report: {c}");
    assert!(c.contains("Which store?"), "a report from before blocks: {c}");
    assert!(c.contains(&format!("tb idea note {i}")), "what's settled goes back as notes: {c}");

    let (_, a) = b.job(&b.post(&format!("/ideas/{i}/talk"), json!({"question": "Last write wins?"})));
    assert_eq!(a["draft"], "Help me decide this one: Last write wins? (Yes / Ask)", "the question is typed in, not sent");
    let (_, a) = b.job(&b.post(&format!("/ideas/{i}/talk"), json!({"question": "Which store?"})));
    assert_eq!(a["draft"], "Help me decide this one: Which store?");
    let (code, _) = b.try_post(&format!("/ideas/{i}/talk"), json!({"question": "Who asked this?"})).unwrap_err();
    assert_eq!(code, 404, "only a question the reports ask");

    b.app.db.update("ideas", &json!(id), fields!["state" => "planned"]).unwrap();
    let (code, msg) = b.try_post(&format!("/ideas/{i}/talk"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("work on the goal instead"), "{msg}");
}

#[test]
fn an_idea_with_no_checkout_has_nowhere_to_talk() {
    let b = board();
    b.app.shared.lock().midna_seen_at = Some(taskboardd::util::now_iso());
    let i = b.post("/ideas", json!({"title": "Dark mode", "project": "elsewhere"}))["ref"].as_str().unwrap().to_string();
    let (code, msg) = b.try_post(&format!("/ideas/{i}/talk"), json!({})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("no checkout"), "{msg}");
}
