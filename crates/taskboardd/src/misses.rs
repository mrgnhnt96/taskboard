//! A miss on a goal's done-when (`tb found --misses-goal`) is a backlog item. When an agent can fix it, the
//! board makes it a task in its goal and queues it, instead of leaving it on the owner's Needs you list; the
//! goal stays open until that task is done (`board::open_miss_rows`), and the hand-off shows in the Handled
//! for you list (`handled.rs`). A miss stays with the owner when its filer marked it as the owner's call
//! (`--needs-owner`), when its own words ask the owner something, or when the board already gave it to an
//! agent once (it came back, so it's the owner's now). `[backlog] misses_to_agent` turns this off.

use serde_json::json;

use crate::app::App;
use crate::util::*;
use crate::{blocks, board, fields, handled, p};

/// Where an open miss goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// An agent fixes it: the board makes it a task in its goal and runs it.
    Agent,
    /// It needs the owner's call, for the reason given; its alert stays on Needs you.
    Owner(&'static str),
    /// Its goal is paused: nothing starts until the owner resumes it, so it waits.
    Later,
    /// Not an open miss in a live goal: nothing to do.
    Skip,
}

/// Where `issue` goes: `goal` is its goal (None when it has none or it's gone), `handed_before` whether
/// the board already made a task from it once.
pub fn pick(issue: &Row, goal: Option<&Row>, handed_before: bool) -> Pick {
    if issue.s("state") != Some("open") || !issue.b("misses_goal") {
        return Pick::Skip;
    }
    let Some(g) = goal.filter(|g| !g.b("archived")) else { return Pick::Skip };
    if issue.b("needs_owner") {
        return Pick::Owner("its filer marked it as the owner's call");
    }
    if handed_before {
        return Pick::Owner("an agent already had it once and it came back");
    }
    if [issue.st("title"), issue.st("blocks_why")].iter().any(|t| crate::reports::asks_owner(t)) {
        return Pick::Owner("it asks the owner something");
    }
    if g.b("paused") {
        return Pick::Later;
    }
    Pick::Agent
}

/// Whether the board already made a task from issue `id` (it hands a miss to an agent once).
fn handed_before(app: &App, id: i64) -> Result<bool> {
    Ok(app.db.count("SELECT COUNT(*) FROM issue_events WHERE issue_id = ? AND who = ? AND kind = 'task'", p![id, board::BOARD])? > 0)
}

/// [`pick`] for issue `id`, reading its goal and history.
pub fn pick_for(app: &App, id: i64) -> Result<Pick> {
    let issue = board::get_issue(app, id)?;
    let goal = issue.i("goal_id").map(|g| board::find_goal(app, Some(g))).transpose()?.flatten();
    Ok(pick(&issue, goal.as_ref(), handed_before(app, id)?))
}

/// A miss was just filed (or an open item was marked as one): an agent gets it when it can fix it, else it
/// goes to the owner as an alert. Returns what became of it for the filer when an agent got it.
pub fn route(app: &App, issue: i64, goal_id: i64, title: &str) -> Result<Option<String>> {
    if app.cfg.backlog.misses_to_agent && pick_for(app, issue)? == Pick::Agent {
        let t = hand_off(app, &board::get_issue(app, issue)?)?;
        return Ok(Some(format!(
            "An agent fixes it: the board made it {}, queued in the goal, which stays open until that's done.",
            rf("task", t)
        )));
    }
    blocks::miss_alert(app, issue, goal_id, title).map(|_| None)
}

/// Makes the miss a task in its goal, queued so it runs whether or not the goal is running, and records it
/// for the Handled for you list. Making it a task brings its alert down (`board::miss_settled`).
pub fn hand_off(app: &App, issue: &Row) -> Result<i64> {
    let id = issue.id();
    let b = rf("issue", id);
    let mut f = crate::api::issue_task_fields(app, issue, board::BOARD);
    f["goal_id"] = issue.v("goal_id");
    f["status"] = json!("queued");
    f["priority"] = json!("normal");
    f["pickup"] = json!({"mode": "queue"});
    let c = crate::ops::new_task(app, &f, board::BOARD, Some(&format!("Added from {b}, a miss on the goal's done-when an agent can fix")))?;
    let Some(tid) = c["id"].as_i64() else { return err(500, format!("Making {b} a task returned no task.")) };
    let t = rf("task", tid);
    crate::api::issue_tasked(app, id, tid, board::BOARD, &format!("Made into {t}, queued in its goal for an agent to fix; the goal stays open until it's done."))?;
    let text = format!("Gave {b} to an agent instead of asking you: it misses the goal's done-when ({}), and {t} fixes it", one_line(&issue.st("title"), 120));
    handled::record(app, tid, "misses", &text, json!({"item": "miss", "issue": id, "goal": issue.v("goal_id"), "verdict": "agent", "was": issue.st("title")}))?;
    Ok(tid)
}

/// Gives every open miss an agent can fix to one: the ones on Needs you from before the board did this,
/// and ones whose goal was paused when they were filed.
pub fn sweep(app: &App) -> Result<Vec<i64>> {
    if !app.cfg.backlog.misses_to_agent {
        return Ok(vec![]);
    }
    let open = app.db.q(
        "SELECT i.id FROM issues i JOIN goals g ON g.id = i.goal_id WHERE i.misses_goal = 1 AND i.state = 'open' \
         AND COALESCE(i.needs_owner, 0) = 0 AND COALESCE(g.archived, 0) = 0 AND COALESCE(g.paused, 0) = 0 ORDER BY i.id",
        p![],
    )?;
    let mut made = vec![];
    for r in open {
        if pick_for(app, r.id())? == Pick::Agent {
            made.push(hand_off(app, &board::get_issue(app, r.id())?)?);
        }
    }
    Ok(made)
}

/// Marks an issue as the owner's call, so it stays on Needs you (`tb found --misses-goal --needs-owner`).
pub fn mark_owner(app: &App, issue: i64) -> Result<()> {
    app.db.update("issues", &json!(issue), fields!["needs_owner" => 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(v: serde_json::Value) -> Row {
        v.as_object().unwrap().clone()
    }

    fn miss(more: serde_json::Value) -> Row {
        let mut r = row(json!({"id": 7, "state": "open", "misses_goal": 1, "goal_id": 3, "title": "Sign-in fails on Android", "blocks_why": "Sign-in fails on Android"}));
        r.extend(row(more));
        r
    }

    fn goal(more: serde_json::Value) -> Row {
        let mut r = row(json!({"id": 3, "name": "Phone app"}));
        r.extend(row(more));
        r
    }

    #[test]
    fn an_open_miss_in_a_live_goal_goes_to_an_agent() {
        assert_eq!(pick(&miss(json!({})), Some(&goal(json!({}))), false), Pick::Agent);
    }

    #[test]
    fn one_marked_as_the_owners_call_stays_with_the_owner() {
        assert!(matches!(pick(&miss(json!({"needs_owner": 1})), Some(&goal(json!({}))), false), Pick::Owner(_)));
    }

    #[test]
    fn one_an_agent_already_had_stays_with_the_owner() {
        assert!(matches!(pick(&miss(json!({})), Some(&goal(json!({}))), true), Pick::Owner(_)));
    }

    #[test]
    fn one_that_asks_the_owner_something_stays_with_the_owner() {
        let asks = miss(json!({"title": "Keep the old sign-in screen or drop it?"}));
        assert!(matches!(pick(&asks, Some(&goal(json!({}))), false), Pick::Owner(_)));
        let why = miss(json!({"blocks_why": "Your call: ship without offline mode"}));
        assert!(matches!(pick(&why, Some(&goal(json!({}))), false), Pick::Owner(_)));
    }

    #[test]
    fn a_paused_goal_holds_it_until_resumed() {
        assert_eq!(pick(&miss(json!({})), Some(&goal(json!({"paused": 1}))), false), Pick::Later);
    }

    #[test]
    fn the_owners_call_wins_over_a_paused_goal() {
        assert!(matches!(pick(&miss(json!({"needs_owner": 1})), Some(&goal(json!({"paused": 1}))), false), Pick::Owner(_)));
    }

    #[test]
    fn anything_but_an_open_miss_in_a_live_goal_is_skipped() {
        let g = goal(json!({}));
        assert_eq!(pick(&miss(json!({"misses_goal": 0})), Some(&g), false), Pick::Skip, "not a miss");
        assert_eq!(pick(&miss(json!({"state": "task"})), Some(&g), false), Pick::Skip, "already a task");
        assert_eq!(pick(&miss(json!({"state": "drop"})), Some(&g), false), Pick::Skip, "closed");
        assert_eq!(pick(&miss(json!({})), None, false), Pick::Skip, "no goal");
        assert_eq!(pick(&miss(json!({})), Some(&goal(json!({"archived": 1}))), false), Pick::Skip, "archived goal");
    }
}
