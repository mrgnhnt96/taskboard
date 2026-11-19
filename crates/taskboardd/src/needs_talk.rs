//! Talking the Needs you list over: the home page's Talk it over button opens a Claude terminal with
//! everything on the list in its context, for the owner to work out with it what to do first. In the
//! filtered project's checkout when the page shows one project, the home folder otherwise.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, dig, home, midna};

/// The context goes on Claude's command line; a very long list is cut short.
const CONTEXT_MAX: usize = 60_000;

/// `POST /home/talk {project?}`: a terminal to talk the list over in. Refused while Midna is down.
pub fn start(app: &App, body: &Value) -> Result<Value> {
    let project = body_str(body, "project");
    let project = Some(project.trim()).filter(|p| !p.is_empty() && *p != "all").map(str::to_string);
    if !midna::up(app) {
        return err(409, "Midna isn't running, so the board can't open a terminal.");
    }
    let cwd = match project.as_deref().map(|p| dig::checkout(app, p)).transpose()?.flatten() {
        Some(p) => p,
        None => dirs::home_dir().unwrap_or_default(),
    };
    let page = home::page(app, "all")?;
    let title = match &project {
        Some(p) => format!("Needs you: {p}"),
        None => "Needs you".to_string(),
    };
    let args = json!({"cwd": cwd, "title": short(&title, 40), "prompt": "", "draft": "What needs me, and what should I do first?",
                      "context": context(&app.cfg.owner, &page, project.as_deref()), "focus": true, "queue": false});
    let j = app.db.tx(|| board::create_job(app, "agent", args, None, "needs_talk", Some(json!({"project": project}))))?;
    Ok(json!({"ok": true, "job": rf("job", j)}))
}

fn st<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("").trim()
}

fn first<'a>(xs: &[&'a str]) -> &'a str {
    xs.iter().copied().find(|x| !x.is_empty()).unwrap_or("")
}

/// The item's project: its task's, else the alert's own.
fn project_of(a: &Value) -> &str {
    first(&[st(&a["card"], "project"), st(a, "project")])
}

/// One line for a Needs you item: its handle for `tb needs show`, what it is, what it asks, and its
/// task, goal and project.
pub fn item_line(a: &Value) -> String {
    let card = &a["card"];
    let task = first(&[st(a, "task"), st(card, "ref")]).replacen('H', "T", 1);
    let goal = first(&[st(a, "goal"), st(&card["goal"], "ref")]);
    let handle = first(&[task.as_str(), st(a, "id"), goal]);
    let asks = st(card, "status") == "needs" && !st(card, "question").is_empty();
    let text = if asks { st(card, "question") } else { st(a, "text") };
    let title = first(&[st(a, "title"), st(card, "title"), text, st(a, "goal_name")]);
    let mut line = format!("- {handle}: {}", one_line(title, 200));
    if !text.is_empty() && text != title {
        line.push_str(&format!(" — {}", one_line(text, 400)));
    }
    let goal_name = st(a, "goal_name");
    let mut about: Vec<String> = vec![];
    if !goal.is_empty() && handle != goal {
        about.push(if goal_name.is_empty() { goal.to_string() } else { format!("{goal} {goal_name}") });
    }
    if !project_of(a).is_empty() {
        about.push(project_of(a).to_string());
    }
    if a["waiting"] == true {
        about.push("an agent waits on it".into());
    }
    if !about.is_empty() {
        line.push_str(&format!(" ({})", about.join(", ")));
    }
    line
}

/// A goal held after a wave for review, as its line: "G3: Sync · Wave 1 is done. Review it, then continue (app)".
pub fn stop_line(e: &Value) -> Option<String> {
    let g = &e["goal"];
    let line = st(g, "stopped");
    if g["paused"] == true || !line.contains(" is done") {
        return None;
    }
    let gr = match st(g, "ref") {
        "" => format!("G{}", g["id"].as_i64()?),
        r => r.to_string(),
    };
    let project = st(e, "project");
    let project = if project.is_empty() { String::new() } else { format!(" ({project})") };
    Some(format!("- {gr}: {} · {line}{project}", st(g, "name")))
}

/// What Claude is told: why it's here, the list as the owner saw it, and where to look for more.
pub fn context(owner: &str, page: &Value, project: Option<&str>) -> String {
    let only = |a: &&Value| project.is_none_or(|p| project_of(a) == p);
    let needs: Vec<&Value> = page["needs"].as_array().map(|n| n.iter().filter(only).collect()).unwrap_or_default();
    let stops: Vec<String> = page["goals"].as_array().into_iter().flatten().filter(only).filter_map(stop_line).collect();
    let flag = project.map(|p| format!(" --project {p}")).unwrap_or_default();
    let mut parts = vec![format!(
        "{owner} opened this terminal from the Needs you page on the task board to talk over what's waiting on them and work \
         out what to do about it. Help them see what matters most, what each item needs from them and what to do first. \
         Look in the code or on the board as you need, but don't change code. Wait for {owner}; don't start on anything by \
         yourself. You aren't on a task: don't take one. Act on an item (answer, restart, dismiss, snooze…) only on \
         {owner}'s word, through tb needs."
    )];
    let scope = project.map(|p| format!(" for {p}")).unwrap_or_default();
    if needs.is_empty() && stops.is_empty() {
        parts.push(format!("Nothing needs {owner}{scope} right now."));
    } else {
        let lines: Vec<String> = needs.iter().map(|a| item_line(a)).chain(stops).collect();
        parts.push(format!("What needs {owner}{scope}, as the page showed it:\n{}", lines.join("\n")));
    }
    let later = page["later_count"].as_i64().unwrap_or(0);
    if later > 0 && project.is_none() {
        parts.push(format!("{later} more put off for later (snoozed); they come back on their own."));
    }
    parts.push(format!(
        "For the list as it stands now: tb needs{flag}. One item in full, with its options: tb needs show <item>. What tb \
         needs can do about one: tb needs --help."
    ));
    clip(&parts.join("\n\n"), CONTEXT_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Value {
        json!({"needs": [
            {"id": "task:T12", "task": "T12", "text": "", "goal": "G3", "goal_name": "Sync", "waiting": true,
             "card": {"ref": "T12", "title": "Add sync", "project": "app", "status": "needs", "question": "Use the old API?"}},
            {"id": "a7", "text": "Master build failed", "project": "api", "card": null, "goal_name": null},
        ], "later_count": 2, "goals": [
            {"project": "app", "goal": {"ref": "G5", "name": "Picto", "stopped": "Wave 1 is done. Review it, then continue"}},
            {"project": "app", "goal": {"ref": "G6", "name": "Busy", "stopped": "T9 in wave 2 failed"}},
        ]})
    }

    #[test]
    fn the_context_lists_every_item_with_its_handle_and_question() {
        let c = context("Morgan", &page(), None);
        assert!(c.contains("- T12: Add sync — Use the old API? (G3 Sync, app, an agent waits on it)"), "{c}");
        assert!(c.contains("- a7: Master build failed (api)"), "{c}");
        assert!(c.contains("- G5: Picto · Wave 1 is done. Review it, then continue (app)"), "{c}");
        assert!(!c.contains("G6"), "only a finished wave is a stop: {c}");
        assert!(c.contains("2 more put off for later"), "{c}");
        assert!(c.contains("tb needs show <item>"), "{c}");
    }

    #[test]
    fn a_project_keeps_only_its_items() {
        let c = context("Morgan", &page(), Some("api"));
        assert!(c.contains("- a7:"), "{c}");
        assert!(!c.contains("T12") && !c.contains("G5"), "{c}");
        assert!(c.contains("tb needs --project api"), "{c}");
        assert!(!c.contains("put off"), "{c}");
    }

    #[test]
    fn an_empty_list_says_so() {
        let c = context("Morgan", &json!({"needs": []}), Some("web"));
        assert!(c.contains("Nothing needs Morgan for web right now."), "{c}");
    }
}
