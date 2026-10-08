//! Sample data for trying the page: `taskboardd seed --data /tmp/tb-dev`.

use serde_json::json;

use crate::app::App;
use crate::util::*;
use crate::{board, fields, ops, p};

pub fn seed(app: &App) -> Result<()> {
    if app.db.count("SELECT COUNT(*) FROM tasks", p![])? > 0 {
        return err(409, "That board already has tasks; seed an empty data folder.");
    }
    app.db.tx(|| {
        app.db.set_setting(
            "midna_projects",
            Some(&jdumps(&json!([{"name": "webapp", "path": "/tmp/webapp"}, {"name": "api", "path": "/tmp/api"}]))),
        )?;
        let g = ops::new_goal(
            app,
            &json!({"name": "Sign-in with passkeys", "project": "webapp", "outcome": "users can sign in with a passkey on web",
                    "tldr": "Add WebAuthn passkeys next to passwords.", "max_terminals": 2}),
        )?;
        let gid = g["id"].as_i64().unwrap_or(0);
        board::add_goal_note(app, gid, "decision", "Keep passwords working; passkeys are an extra option.", Some("you"), true, None)?;
        board::add_goal_note(app, gid, "finding", "The session cookie is set in middleware/auth.ts.", Some("T1"), false, None)?;
        let mk = |title: &str, detail: &str, status: &str, goal: Option<i64>| -> Result<i64> {
            let c = ops::new_task(
                app,
                &json!({"title": title, "detail": detail, "project": "webapp", "goal_id": goal,
                        "status": if status == "planned" { "planned" } else { "queued" }, "pickup": {"mode": "manual"}}),
                board::OWNER,
                None,
            )?;
            let id = c["id"].as_i64().unwrap_or(0);
            if status != "queued" && status != "planned" {
                board::update_task(app, id, fields!["status" => status, "started_at" => now_iso()])?;
            }
            Ok(id)
        };
        let t1 = mk("Add the WebAuthn registration endpoint", "POST /auth/passkeys/register with challenge storage.", "done", Some(gid))?;
        board::update_task(app, t1, fields!["summary" => "Registration endpoint and tests are in.", "finished_at" => now_iso(), "latest" => "Registration endpoint and tests are in."])?;
        board::log_event(app, t1, "T1 Add the WebAuthn", "status", "Marked done: Registration endpoint and tests are in.")?;
        let t2 = mk("Sign in with a passkey", "Add the login ceremony and the button on the sign-in page.", "working", Some(gid))?;
        board::update_task(app, t2, fields!["latest" => "Wiring the assertion check into the session middleware.", "session_name" => "T2 Sign in with a passkey"])?;
        let t3 = mk("Settings page: manage passkeys", "List, rename and remove passkeys.", "needs", Some(gid))?;
        board::update_task(app, t3, fields!["needs_reason" => "question", "question" => "Should removing the last passkey require a password re-check?"])?;
        mk("Docs: passkey support", "Explain passkeys in the help centre.", "planned", Some(gid))?;
        mk("Upgrade the HTTP client", "Move to the new major version and fix the call sites.", "queued", None)?;
        let now = now_iso();
        app.db.insert(
            "issues",
            fields!["project" => "webapp", "goal_id" => gid, "kind" => "gap", "title" => "No test for an expired challenge",
                    "said" => "“The register endpoint has no test for an expired challenge.”", "how" => "The T1 terminal reported it while working on T1.",
                    "source" => "terminal", "found_by_task" => t1, "found_by_name" => "T1 Add the WebAuthn", "state" => "open",
                    "snapshot" => jdumps(&json!({"task": "T1 · Add the WebAuthn registration endpoint", "branch": "feature/passkeys"})),
                    "created_at" => now, "updated_at" => now],
        )?;
        Ok(())
    })
}
