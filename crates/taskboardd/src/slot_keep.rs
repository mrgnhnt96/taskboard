//! Keeps the worktree slot pool (`slots.rs`) out of Midna's cleanup after a closed terminal. That
//! cleanup removes every worktree the terminal was ever in, sparing one only while another terminal's
//! folder is in it or it has changes. A slot's agent works in it from a terminal that stays in the
//! shared checkout, so a slot taken a moment before, still clean, went with its new branch while its
//! task held it. Closing a slot's own terminal with cleanup off doesn't cover that terminal, nor one
//! the owner closes.
//!
//! Every slot's path goes in Midna's `cleanup.keep`, and comes out once the slot leaves the pool. Only
//! the paths the board put there are taken out.

use serde_json::{json, Value};

use crate::app::App;
use crate::midna::{self, MidnaError};
use crate::p;
use crate::util::*;

const KEY: &str = "cleanup.keep";
/// The slot paths the board last put in `cleanup.keep`, so a sync with no slot made or removed asks
/// Midna nothing, and a removed slot's path is known to be the board's to take out.
const KEPT_KEY: &str = "midna_cleanup_kept_slots";

/// `keep` with `slots` added and the paths in `before` that aren't slots any more taken out. None if
/// it has that already.
pub fn with_slots(keep: &[String], before: &[String], slots: &[String]) -> Option<Vec<String>> {
    let mut out: Vec<String> = keep.iter().filter(|k| slots.contains(k) || !before.contains(k)).cloned().collect();
    out.extend(slots.iter().filter(|s| !keep.contains(s)).cloned());
    (out != keep).then_some(out)
}

fn strings(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
}

/// Puts the pool's slots in Midna's `cleanup.keep` when they've changed since the last time.
pub fn sync(app: &App) {
    let Ok(rows) = app.db.q("SELECT path FROM worktree_slots ORDER BY repo, n", p![]) else { return };
    let slots: Vec<String> = rows.iter().map(|r| r.st("path")).collect();
    let before = app.db.get_setting(KEPT_KEY).ok().flatten().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| strings(&v));
    if before.as_ref() == Some(&slots) {
        return;
    }
    let keep = match midna::call(app, "settings.get", json!({"key": KEY})) {
        Ok(v) => match strings(&v["value"]) {
            Some(keep) => keep,
            None => return,
        },
        Err(_) => return,
    };
    if let Some(next) = with_slots(&keep, before.as_deref().unwrap_or_default(), &slots) {
        match midna::call(app, "settings.set", json!({"key": KEY, "value": next})) {
            Err(MidnaError::Down(_)) => return,
            Ok(_) => app.info(format!("midna: cleanup.keep spares the {} worktree slots", slots.len())),
            Err(e) => app.info(format!("midna: asked cleanup.keep to spare the worktree slots ({e})")),
        }
    }
    let _ = app.db.set_setting(KEPT_KEY, Some(&json!(slots).to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn slots_go_after_the_owners_entries() {
        let keep = v(&["main", "release/*"]);
        let slots = v(&["/r/.claude/worktrees/slot-1", "/r/.claude/worktrees/slot-2"]);
        assert_eq!(with_slots(&keep, &[], &slots), Some(v(&["main", "release/*", "/r/.claude/worktrees/slot-1", "/r/.claude/worktrees/slot-2"])));
    }

    #[test]
    fn nothing_to_change_when_every_slot_is_kept() {
        let keep = v(&["/r/.claude/worktrees/slot-1", "main"]);
        assert_eq!(with_slots(&keep, &v(&["/r/.claude/worktrees/slot-1"]), &v(&["/r/.claude/worktrees/slot-1"])), None);
    }

    #[test]
    fn a_removed_slot_comes_out_but_the_owners_paths_stay() {
        let keep = v(&["main", "/r/.claude/worktrees/slot-1", "/r/.claude/worktrees/slot-2", "/r/.claude/worktrees/mine"]);
        let before = v(&["/r/.claude/worktrees/slot-1", "/r/.claude/worktrees/slot-2"]);
        assert_eq!(
            with_slots(&keep, &before, &v(&["/r/.claude/worktrees/slot-1"])),
            Some(v(&["main", "/r/.claude/worktrees/slot-1", "/r/.claude/worktrees/mine"]))
        );
    }
}
