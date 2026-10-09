//! Midna's status bar: the selected terminal's task, its goal and its line, drawn by the bundled
//! `plugin/task-board/bin/midna-status` script (`tb peek --midna`).
//!
//! What it shows is the board's: the goal and the titles are on unless `tb status-bar` turns them
//! off. The board also asks Midna, once per copy of the script, to put it on the status bar; Midna
//! has the owner approve that, since adding a script there is theirs to say.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::app::App;
use crate::midna::{self, MidnaError};
use crate::util::*;

const KEY: &str = "status_bar";
/// The script path the board last asked Midna to show, so it asks once and not on every sync.
const OFFERED_KEY: &str = "status_bar_offered";
const SCRIPT: &str = "plugin/task-board/bin/midna-status";
/// Most queued tasks the bar names one by one; the rest are a count.
const LINE_MAX: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// The task's goal, before the task.
    pub goal: bool,
    /// Task and goal titles after their refs (off: refs only; the tooltip still has the titles).
    pub title: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { goal: true, title: true }
    }
}

impl Options {
    pub fn to_json(self) -> Value {
        json!({"goal": self.goal, "title": self.title})
    }

    fn from_json(v: &Value) -> Self {
        let d = Options::default();
        Options { goal: v["goal"].as_bool().unwrap_or(d.goal), title: v["title"].as_bool().unwrap_or(d.title) }
    }

    /// The options a whoami carries (`status_bar`); the defaults from a board that doesn't send them.
    pub fn from_whoami(v: &Value) -> Self {
        Options::from_json(&v["status_bar"])
    }
}

pub fn get(app: &App) -> Options {
    let saved = app.db.get_setting(KEY).ok().flatten().and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
    Options::from_json(&saved)
}

/// `tb status-bar`: `goal` and `title` as true or false; `reset` goes back to the defaults.
pub fn set(app: &App, body: &Value) -> Result<Options> {
    let mut o = if as_bool(body.get("reset"), false) { Options::default() } else { get(app) };
    for (k, slot) in [("goal", &mut o.goal), ("title", &mut o.title)] {
        match body.get(k) {
            None | Some(Value::Null) => {}
            Some(Value::Bool(b)) => *slot = *b,
            Some(_) => return err(400, format!("{k} must be on or off.")),
        }
    }
    app.db.set_setting(KEY, Some(&o.to_json().to_string()))?;
    Ok(o)
}

pub fn line(o: Options) -> String {
    let on = |b: bool| if b { "on" } else { "off" };
    format!("Goal: {}\nTitles: {}", on(o.goal), on(o.title))
}

pub fn state(app: &App) -> Value {
    let o = get(app);
    let mut out = o.to_json();
    out["line"] = json!(line(o));
    out
}

/// A terminal's whoami (`GET /whoami`) as Midna status-bar segments (`midna explain scripts`): its
/// goal, its task (or a PR it's visiting), then its line. Each opens on the board when clicked.
pub fn segments(v: &Value, o: Options) -> Value {
    let link = |kind: &str, r: &str| format!("taskboard://{kind}/{r}");
    let named = |r: &str, title: &str, n: usize| if o.title && !title.is_empty() { format!("{r} {}", clip(title, n)) } else { r.to_string() };
    let mut segs = vec![];
    if let Some(r) = v["task"]["ref"].as_str() {
        let t = &v["task"];
        let title = t["title"].as_str().unwrap_or("");
        let (label, tone, icon) = match (t["failed"].as_bool().unwrap_or(false), t["status"].as_str().unwrap_or("")) {
            (true, _) => ("Failed", "err", "cross"),
            (_, "working") => ("Working", "work", "bolt"),
            (_, "needs") => ("Needs you", "need", "bell"),
            (_, "done") => ("Done", "ok", "check"),
            (_, "queued") => ("Queued", "dim", "dot"),
            (_, "planned") => ("Planned", "dim", "dot"),
            (_, other) => (other, "dim", "dot"),
        };
        let mut tip = format!("{r} “{title}” · {label}");
        if let Some(g) = t["goal"]["ref"].as_str() {
            let name = t["goal"]["name"].as_str().unwrap_or("");
            tip.push_str(&format!("\n{g} “{name}”"));
            if o.goal {
                segs.push(json!({"text": named(g, name, 24), "tone": "accent", "tooltip": format!("{g} “{name}”"), "link": link("goal", g)}));
            }
        }
        if let Some(why) = t["needs_reason"].as_str().or(t["question"].as_str()).filter(|s| !s.is_empty()) {
            tip.push_str(&format!("\n{why}"));
        }
        segs.push(json!({"text": named(r, title, 40), "tone": tone, "icon": icon, "tooltip": tip, "link": link("task", r)}));
    } else if let Some(r) = v["visiting"]["ref"].as_str() {
        let text = match v["visiting"]["pr"]["num"].as_i64() {
            Some(n) => format!("{r} PR #{n}"),
            None => format!("{r} PR"),
        };
        let tip = format!("Visiting the PR of {r} “{}”", v["visiting"]["title"].as_str().unwrap_or(""));
        segs.push(json!({"text": text, "tone": "accent", "icon": "pr", "tooltip": tip, "link": link("task", r)}));
    }
    let line = v["line"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    for (i, x) in line.iter().take(LINE_MAX).enumerate() {
        let r = x["ref"].as_str().unwrap_or("");
        let what = if x["kind"] == "resume" { "To resume here" } else { "Queued here" };
        let mut s = json!({"text": r, "tone": "dim", "tooltip": format!("{what}: “{}”", x["title"].as_str().unwrap_or("")), "link": link("task", r)});
        if i == 0 {
            s["icon"] = json!("play");
        } else {
            s["join"] = json!(true);
        }
        segs.push(s);
    }
    if line.len() > LINE_MAX {
        let rest: Vec<&str> = line[LINE_MAX..].iter().filter_map(|x| x["ref"].as_str()).collect();
        segs.push(json!({"text": format!("+{}", rest.len()), "tone": "dim", "join": true, "tooltip": format!("Also in line: {}", rest.join(", "))}));
    }
    Value::Array(segs)
}

/// The script in the app this daemon runs from (`…/Taskboard.app/Contents/MacOS/taskboardd`), if
/// it's there. None from a checkout or a test build.
fn bundled_script() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let contents = exe.parent()?.parent()?;
    if contents.file_name()? != "Contents" || !contents.parent()?.extension().is_some_and(|e| e == "app") {
        return None;
    }
    Some(contents.join("Resources").join(SCRIPT)).filter(|p| p.is_file())
}

/// Midna's status-bar items with `script` on it: an older copy of the script (a checkout, an app
/// that moved) is swapped for it in place, else it goes just before `update`. None if it's there.
pub fn with_script(items: &[String], script: &str) -> Option<Vec<String>> {
    if items.iter().any(|i| i == script) {
        return None;
    }
    let mut out = items.to_vec();
    match out.iter().position(|i| i.ends_with(&format!("/{SCRIPT}"))) {
        Some(at) => out[at] = script.to_string(),
        None => {
            let at = out.iter().position(|i| i == "update").unwrap_or(out.len());
            out.insert(at, script.to_string());
        }
    }
    Some(out)
}

/// Puts the bundled script on Midna's status bar: asks once per copy of it. Midna turns the ask
/// into a confirmation for the owner, so a no stays a no.
pub fn offer(app: &App) {
    if std::env::var_os("TASKBOARD_NO_TB_LINK").is_some() {
        return;
    }
    let Some(script) = bundled_script() else { return };
    offer_script(app, &script);
}

fn offer_script(app: &App, script: &Path) {
    let script = script.to_string_lossy().to_string();
    if app.db.get_setting(OFFERED_KEY).ok().flatten().as_deref() == Some(script.as_str()) {
        return;
    }
    let items = match midna::call(app, "settings.get", json!({"key": "ui.status.items"})) {
        Ok(v) => v["value"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default(),
        Err(_) => return,
    };
    if let Some(next) = with_script(&items, &script) {
        match midna::call(app, "settings.set", json!({"key": "ui.status.items", "value": next})) {
            Err(MidnaError::Down(_)) => return,
            Ok(_) => app.info("midna: put the task-board item on the status bar"),
            Err(e) => app.info(format!("midna: asked to put the task-board item on the status bar ({e})")),
        }
    }
    let _ = app.db.set_setting(OFFERED_KEY, Some(&script));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whoami() -> Value {
        let line: Vec<Value> = (40..45).map(|n| json!({"ref": format!("T{n}"), "title": format!("Job {n}"), "kind": if n == 40 { "resume" } else { "queued" }})).collect();
        json!({"task": {"ref": "T36", "title": "Show the tasks", "status": "needs", "failed": false,
                        "goal": {"ref": "G2", "name": "Polish"}, "needs_reason": "Pick a color"},
               "line": line, "visiting": null})
    }

    #[test]
    fn the_goal_then_the_task_then_its_line() {
        let s = segments(&whoami(), Options::default());
        let s = s.as_array().unwrap();
        assert_eq!(s[0]["text"], "G2 Polish");
        assert_eq!(s[0]["link"], "taskboard://goal/G2");
        assert_eq!(s[1]["text"], "T36 Show the tasks");
        assert_eq!(s[1]["tone"], "need");
        assert_eq!(s[1]["link"], "taskboard://task/T36");
        assert_eq!(s[1]["tooltip"], "T36 “Show the tasks” · Needs you\nG2 “Polish”\nPick a color");
        assert_eq!(s[2]["text"], "T40");
        assert_eq!(s[2]["tooltip"], "To resume here: “Job 40”");
        assert!(s[2]["join"].is_null() && s[3]["join"] == true);
        assert_eq!(s.len(), 6, "three in the line by name, then a count");
        assert_eq!(s[5]["text"], "+2");
        assert_eq!(s[5]["tooltip"], "Also in line: T43, T44");
    }

    #[test]
    fn the_goal_and_titles_can_be_left_off() {
        let s = segments(&whoami(), Options { goal: false, title: false });
        assert_eq!(s[0]["text"], "T36", "refs only");
        assert!(s[0]["tooltip"].as_str().unwrap().contains("“Show the tasks”"), "the title is still on hover");
        let s = segments(&whoami(), Options { goal: true, title: false });
        assert_eq!((s[0]["text"].as_str(), s[1]["text"].as_str()), (Some("G2"), Some("T36")));
    }

    #[test]
    fn a_visited_pr_and_nothing_for_an_idle_terminal() {
        let v = json!({"task": null, "line": [], "visiting": {"ref": "T9", "title": "Fix login", "pr": {"num": 12}}});
        assert_eq!(segments(&v, Options::default())[0]["text"], "T9 PR #12");
        assert_eq!(segments(&json!({"task": null, "line": [], "visiting": null}), Options::default()), json!([]));
    }

    #[test]
    fn the_script_goes_before_update_or_replaces_an_older_copy() {
        let items = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let app = "/Applications/Taskboard.app/Contents/Resources/plugin/task-board/bin/midna-status";
        assert_eq!(with_script(&items(&["daemon", "spacer", "update", "keys"]), app).unwrap(), items(&["daemon", "spacer", app, "update", "keys"]));
        assert_eq!(with_script(&items(&["daemon"]), app).unwrap(), items(&["daemon", app]));
        let old = "/Users/me/taskboard/plugin/task-board/bin/midna-status";
        assert_eq!(with_script(&items(&["spacer", old, "update"]), app).unwrap(), items(&["spacer", app, "update"]));
        assert_eq!(with_script(&items(&["spacer", app]), app), None);
    }
}
