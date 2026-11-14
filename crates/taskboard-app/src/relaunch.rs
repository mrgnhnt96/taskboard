//! Restart to update keeps what's typed: on the way out (only while an update is being applied)
//! the idea draft and the task panel's drafts are saved, and the new app puts them back and says
//! which version it's on now.
use crate::app::MainWindow;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::HashMap;

const KEY: &str = "tb.restart.drafts";

/// What's typed, as saved: `{"idea": {"title", "detail", "project"}, "panel": {key: text}}`, or
/// None when nothing is.
pub fn kept(idea: Option<(String, String, String)>, panel: HashMap<String, String>) -> Option<Value> {
    if idea.is_none() && panel.is_empty() {
        return None;
    }
    let mut v = json!({"panel": panel});
    if let Some((title, detail, project)) = idea {
        v["idea"] = json!({"title": title, "detail": detail, "project": project});
    }
    Some(v)
}

/// Quitting: save the drafts when it's for an update.
pub fn save(m: &MainWindow, cx: &App) {
    if !matches!(crate::updater::state(), crate::updater::State::Applying { .. }) {
        return;
    }
    match kept(crate::ui::ideas::draft(m, cx), crate::ui::task_panel::drafts()) {
        Some(v) => crate::prefs::set(KEY, v),
        None => crate::prefs::remove(KEY),
    }
}

/// Back after an update: put the drafts back and say so, with the version now running.
pub fn restore(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let saved = crate::prefs::get(KEY);
    crate::prefs::remove(KEY);
    let mut back = false;
    if let Some(v) = saved {
        if let Some(i) = v.get("idea").filter(|i| i.is_object()) {
            let s = |k: &str| i[k].as_str().unwrap_or("").to_string();
            crate::ui::ideas::restore_draft(m, &s("title"), &s("detail"), &s("project"), cx);
            back = true;
        }
        if let Some(p) = v.get("panel").and_then(Value::as_object).filter(|p| !p.is_empty()) {
            crate::ui::task_panel::restore_drafts(p.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect());
            back = true;
        }
    }
    let Some((version, at)) = crate::updater::take_updated() else { return };
    crate::updater::set_updated(Some((version.clone(), at)));
    m.toast(updated_line(&version, back), false, cx);
}

/// "Updated to Taskboard 1.2.0. What you were typing is back."
pub fn updated_line(version: &str, drafts_back: bool) -> String {
    let back = if drafts_back { " What you were typing is back." } else { "" };
    format!("Updated to Taskboard {version}.{back}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn only_what_holds_text_is_kept() {
        assert_eq!(kept(None, HashMap::new()), None);
        let v = kept(Some(("Dark mode".into(), "".into(), "app".into())), HashMap::from([("reply:T4".to_string(), "yes".to_string())])).unwrap();
        assert_eq!(v["idea"]["title"], "Dark mode");
        assert_eq!(v["idea"]["project"], "app");
        assert_eq!(v["panel"]["reply:T4"], "yes");
        assert_eq!(updated_line("1.2.0", true), "Updated to Taskboard 1.2.0. What you were typing is back.");
        assert_eq!(updated_line("1.2.0", false), "Updated to Taskboard 1.2.0.");
    }
}
