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
        seed_waves(app, t2)?;
        seed_pool(app, gid, t2, t3)?;
        seed_today(app, &[(t1, "done"), (t2, "working"), (t3, "needs")])?;
        seed_history(app)?;
        Ok(())
    })
}

/// A goal planned in waves: a review stop it was let past, a wave running, one waiting, a task with
/// no wave, and a task of the passkeys goal that also finishes this one.
fn seed_waves(app: &App, shared: i64) -> Result<()> {
    let g = ops::new_goal(
        app,
        &json!({"name": "Faster sign-in", "project": "webapp", "outcome": "sign-in takes under a second", "tldr": "Cut sign-in latency.",
                "max_terminals": 3, "run_in_order": false}),
    )?;
    let gid = g["id"].as_i64().unwrap_or(0);
    let mk = |title: &str, wave: Option<i64>, status: &str| -> Result<i64> {
        let c = ops::new_task(
            app,
            &json!({"title": title, "detail": "Sample work.", "project": "webapp", "goal_id": gid, "wave": wave, "status": "queued", "pickup": {"mode": "queue"}}),
            board::OWNER,
            None,
        )?;
        let id = c["id"].as_i64().unwrap_or(0);
        if status != "queued" {
            board::update_task(app, id, fields!["status" => status, "started_at" => iso(now_ts() - 3900.0)])?;
        }
        Ok(id)
    };
    for t in [mk("Measure the sign-in path", Some(1), "done")?, mk("Cache the user lookup", Some(1), "done")?] {
        board::update_task(app, t, fields!["finished_at" => iso(now_ts() - 5400.0), "summary" => "Done."])?;
    }
    crate::waves::set_wave(app, gid, 1, fields!["name" => "Measure", "stop_after" => 1, "released_at" => iso(now_ts() - 3600.0)])?;
    let w = mk("Batch the session writes", Some(2), "working")?;
    board::update_task(app, w, fields!["session_name" => "T Batch writes", "updated_at" => iso(now_ts() - 120.0)])?;
    mk("Drop the extra redirect", Some(2), "queued")?;
    crate::waves::set_wave(app, gid, 3, fields!["name" => "Check", "stop_after" => 1])?;
    mk("Re-measure and compare", Some(3), "queued")?;
    mk("Write up the numbers", None, "planned")?;
    crate::shared::add(app, &board::get_task(app, shared)?, &[gid], board::OWNER)?;
    Ok(())
}

/// A small device pool (one device lent to the running passkey task) and the passkeys goal's bits.
fn seed_pool(app: &App, gid: i64, running: i64, asking: i64) -> Result<()> {
    let now = now_iso();
    for (name, tags, focus) in [("iphone-16-sim", json!(["ios", "simulator"]), Some("open -a Simulator")), ("pixel-8", json!(["android", "phone"]), None)] {
        app.db.insert("devices", fields!["name" => name, "tags" => jdumps(&tags), "focus" => focus, "off" => 0, "created_at" => now, "updated_at" => now])?;
    }
    app.db.insert("device_loans", fields!["device" => "iphone-16-sim", "task_id" => running, "at" => now])?;
    app.db.x("INSERT INTO device_needs(owner, needs) VALUES(?, ?)", p![rf("task", running), jdumps(&json!([{"tag": "ios", "n": 1}]))])?;
    for (name, kind, made, task) in [("passkeys", "backend", true, None), ("passkeys.settings", "backend", false, Some(asking)), ("passkey-banner", "local", false, Some(running))] {
        let id = app.db.insert(
            "bits",
            fields!["name" => name, "kind" => kind, "project" => "webapp", "made_at" => if made { Some(now.clone()) } else { None },
                    "made_by" => if made { Some(board::OWNER) } else { None }, "created_at" => now, "updated_at" => now],
        )?;
        app.db.insert("bit_links", fields!["bit_id" => id, "task_id" => task, "goal_id" => if task.is_none() { Some(gid) } else { None }, "at" => now])?;
    }
    Ok(())
}

/// Today's sample tasks started a few hours ago, not the second the board was seeded.
fn seed_today(app: &App, tasks: &[(i64, &str)]) -> Result<()> {
    let now = now_ts();
    let start = crate::days::today().and_hms_opt(0, 0, 0).and_then(|n| chrono::TimeZone::from_local_datetime(&chrono::Local, &n).earliest()).map(|t| t.timestamp() as f64).unwrap_or(now);
    let at = |hours_ago: f64| iso((now - hours_ago * 3600.0).max(start + 60.0));
    for (i, (id, status)) in tasks.iter().enumerate() {
        app.db.x("DELETE FROM task_states WHERE task_id = ?", p![id])?;
        let begun = 3.0 - i as f64 * 0.6;
        app.db.insert("task_states", fields!["task_id" => id, "at" => at(begun), "status" => "working", "failed" => 0, "project" => "webapp"])?;
        match *status {
            "done" => {
                app.db.insert("task_states", fields!["task_id" => id, "at" => at(1.0), "status" => "done", "failed" => 0, "project" => "webapp"])?;
                app.db.x("UPDATE tasks SET human_min = 360 WHERE id = ?", p![id])?;
            }
            "needs" => {
                app.db.insert("task_states", fields!["task_id" => id, "at" => at(0.6), "status" => "needs", "needs_reason" => "question", "failed" => 0, "project" => "webapp"])?;
                app.db.insert("events", fields!["task_id" => id, "at" => at(0.6), "who" => "T3", "kind" => "question", "text" => "Asked: Should removing the last passkey require a password re-check?"])?;
            }
            _ => {}
        }
        for k in 0..3 {
            app.db.insert("events", fields!["task_id" => id, "at" => at(begun - 0.3 - k as f64 * 0.5), "who" => "T", "kind" => "commit", "text" => "Committed: Work in progress"])?;
        }
    }
    Ok(())
}

/// Two weeks of finished sample work, so the Days page has history to show.
fn seed_history(app: &App) -> Result<()> {
    let titles = [
        ("webapp", "Remember the last sign-in method"),
        ("api", "Rate-limit the token endpoint"),
        ("webapp", "Fix the avatar upload on Safari"),
        ("api", "Add pagination to the audit log"),
        ("webapp", "Dark mode for the settings page"),
        ("api", "Retry webhook deliveries with backoff"),
    ];
    let today = crate::days::today();
    let mut n = 0usize;
    for back in 1..=16i64 {
        let day = today - chrono::Duration::days(back);
        if matches!(chrono::Datelike::weekday(&day), chrono::Weekday::Sat | chrono::Weekday::Sun) {
            continue;
        }
        let Some(start) = day.and_hms_opt(9, 0, 0).and_then(|d| chrono::TimeZone::from_local_datetime(&chrono::Local, &d).earliest()) else { continue };
        let base = start.timestamp() as f64;
        for k in 0..(2 + back as usize % 3) {
            let (project, title) = titles[n % titles.len()];
            n += 1;
            let from = base + (k as f64 * 1.7 + (back % 2) as f64 * 0.5) * 3600.0;
            let to = from + (1.0 + ((back as usize + k) % 4) as f64 * 0.6) * 3600.0;
            let id = app.db.insert(
                "tasks",
                fields!["title" => title, "detail" => "Sample work.", "project" => project, "status" => "done", "created_at" => iso(from - 600.0),
                        "updated_at" => iso(to), "started_at" => iso(from), "finished_at" => iso(to), "summary" => "Done.",
                        "human_min" => (((to - from) / 60.0) * (2.5 + (n % 4) as f64 * 0.5)).round() as i64],
            )?;
            app.db.x("DELETE FROM task_states WHERE task_id = ?", p![id])?;
            app.db.insert("task_states", fields!["task_id" => id, "at" => iso(from), "status" => "working", "failed" => 0, "project" => project])?;
            if n % 3 == 0 {
                let ask = from + (to - from) * 0.5;
                app.db.insert("task_states", fields!["task_id" => id, "at" => iso(ask), "status" => "needs", "needs_reason" => "question", "failed" => 0, "project" => project])?;
                app.db.insert("task_states", fields!["task_id" => id, "at" => iso((ask + 600.0 + (n % 4) as f64 * 240.0).min(to - 300.0)), "status" => "working", "failed" => 0, "project" => project])?;
                app.db.insert("events", fields!["task_id" => id, "at" => iso(ask), "who" => "T", "kind" => "question", "text" => "Asked: Which way should this go?"])?;
            }
            app.db.insert("task_states", fields!["task_id" => id, "at" => iso(to), "status" => "done", "failed" => 0, "project" => project])?;
            for c in 0..(1 + n % 3) {
                app.db.insert("events", fields!["task_id" => id, "at" => iso(from + 900.0 + c as f64 * 1200.0), "who" => "T", "kind" => "commit", "text" => "Committed: Sample change"])?;
            }
            if n % 2 == 0 {
                app.db.insert("events", fields!["task_id" => id, "at" => iso(to - 300.0), "who" => "T", "kind" => "status", "text" => format!("Linked PR #{} (acme/{project})", 100 + n)])?;
            }
        }
    }
    Ok(())
}
