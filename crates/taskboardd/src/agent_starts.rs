//! Who may start work from a terminal without the owner's word (`[agent_starts]` in config.toml). By
//! default an agent starts a task or runs a goal only on the owner's word there ([`crate::startword`]).
//! The owner can let agents start work by themselves, for every project or only some:
//!
//! - `ask`: the owner's word is needed, as before.
//! - `own_project`: an agent starts work in its own terminal's project by itself; work in any other
//!   project still needs the word.
//! - `free`: an agent starts any work by itself.
//!
//! The mode is the work's project's own (`[agent_starts.projects]`), else `mode`. Only a terminal's
//! `tb start` reads it: a caller with no terminal is still no one's word.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::app::App;
use crate::p;
use crate::util::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Ask,
    OwnProject,
    Free,
}

/// `[agent_starts]` in config.toml.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AgentStartsConfig {
    /// The mode for every project that doesn't name its own.
    pub mode: Mode,
    /// A project's own mode, by project name.
    pub projects: BTreeMap<String, Mode>,
}

impl AgentStartsConfig {
    /// The mode for work in `project` (none: the default).
    pub fn mode_for(&self, project: Option<&str>) -> Mode {
        project.filter(|p| !p.is_empty()).and_then(|p| self.projects.get(p).copied()).unwrap_or(self.mode)
    }

    /// Whether an agent in a terminal of project `terminal` may start work in project `work` without
    /// the owner's word.
    pub fn allows(&self, work: Option<&str>, terminal: Option<&str>) -> bool {
        match self.mode_for(work) {
            Mode::Ask => false,
            Mode::Free => true,
            Mode::OwnProject => matches!((work, terminal), (Some(w), Some(t)) if !w.is_empty() && w == t),
        }
    }
}

/// The project of terminal `sid`: its task's, else the folder it runs in.
fn terminal_project(app: &App, sid: &str) -> Result<Option<String>> {
    if let Some(t) = crate::board::task_for_session(app, Some(sid))? {
        if let Some(p) = t.s("project").filter(|p| !p.is_empty()) {
            return Ok(Some(p.to_string()));
        }
    }
    let row = app.db.q1("SELECT project FROM sessions WHERE id = ?", p![sid])?;
    Ok(row.and_then(|r| r.s("project").filter(|p| !p.is_empty()).map(str::to_string)))
}

/// Whether terminal `sid` may start work in project `work` by itself (`[agent_starts]`).
pub fn allowed(app: &App, sid: &str, work: Option<&str>) -> Result<bool> {
    let cfg = &app.cfg.agent_starts;
    if cfg.mode == Mode::Ask && cfg.projects.values().all(|m| *m == Mode::Ask) {
        return Ok(false);
    }
    if sid.is_empty() {
        return Ok(false);
    }
    Ok(cfg.allows(work, terminal_project(app, sid)?.as_deref()))
}

/// Whether terminal `sid` may run goal `g` by itself: every project it holds work in allows it.
pub fn allowed_goal(app: &App, sid: &str, g: &Row) -> Result<bool> {
    let projects = crate::board::goal_projects(app, g)?;
    if projects.is_empty() {
        return allowed(app, sid, None);
    }
    for p in &projects {
        if !allowed(app, sid, Some(p))? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(mode: Mode, projects: &[(&str, Mode)]) -> AgentStartsConfig {
        AgentStartsConfig { mode, projects: projects.iter().map(|(p, m)| (p.to_string(), *m)).collect() }
    }

    #[test]
    fn ask_needs_the_word_everywhere() {
        let c = cfg(Mode::Ask, &[]);
        assert!(!c.allows(Some("a"), Some("a")));
    }

    #[test]
    fn free_starts_anything() {
        let c = cfg(Mode::Free, &[]);
        assert!(c.allows(Some("a"), Some("b")));
        assert!(c.allows(None, None));
    }

    #[test]
    fn own_project_starts_only_its_own() {
        let c = cfg(Mode::OwnProject, &[]);
        assert!(c.allows(Some("a"), Some("a")));
        assert!(!c.allows(Some("a"), Some("b")));
        assert!(!c.allows(Some("a"), None));
        assert!(!c.allows(None, None));
    }

    #[test]
    fn a_projects_own_mode_wins() {
        let c = cfg(Mode::Free, &[("work", Mode::Ask), ("home", Mode::OwnProject)]);
        assert!(!c.allows(Some("work"), Some("work")));
        assert!(c.allows(Some("home"), Some("home")));
        assert!(!c.allows(Some("home"), Some("other")));
        assert!(c.allows(Some("other"), Some("home")));
    }

    #[test]
    fn reads_from_toml() {
        let c: AgentStartsConfig = toml::from_str("mode = \"own_project\"\n[projects]\nwork = \"ask\"\n").unwrap();
        assert_eq!(c.mode, Mode::OwnProject);
        assert_eq!(c.mode_for(Some("work")), Mode::Ask);
    }
}
