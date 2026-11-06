//! A done task whose PR was declined holds the tasks that wait for its PR to merge (`tb wait-for
//! --merged`): that merge won't come. Often the work landed another way (a hand-made commit, another
//! PR), so the owner or an agent can say where (`tb task set T<n> --landed <commit>`), which counts as
//! merged. Until then the board raises a Needs-you naming the held waiters and both ways out: say where
//! it landed, or drop the wait (`tb wait-for --drop`).
//!
//! A done task with no PR linked holds its waiters the same way, so it gets the same Needs-you, with one
//! more way out: name the PR its work went through (`tb task set T<n> --pr-link <link>`).

use serde_json::Value;

use crate::app::App;
use crate::util::*;
use crate::waitsfor::{self, landed, pr_declined};
use crate::worktrees::{git, GIT_SECS};
use crate::{board, dispatch, fields, p};

const PREFIX: &str = "declined-waits:";

fn key(id: i64) -> String {
    format!("{PREFIX}{id}")
}

/// The not-done tasks that wait for `t` and can't go on until its PR merges.
pub fn held(app: &App, t: &Row) -> Result<Vec<Row>> {
    let mut out = vec![];
    for w in waitsfor::waiting_on(app, t)? {
        if waitsfor::ids(&w).contains(&t.id()) && !waitsfor::ready_for(app, &w, t.id(), Some(t))? {
            out.push(w);
        }
    }
    Ok(out)
}

fn stuck(t: &Row) -> bool {
    t.s("status") == Some("done") && landed(t).is_none() && if t.i("pr_num").is_some() { pr_declined(t) } else { !t.b("failed") && !has(t.s("pr_url")) }
}

/// `tb submit` is refused when it would leave a task that waits for this one's merge with no PR to wait for.
pub fn refuse_stranding(app: &App, t: &Row) -> Result<()> {
    if has(t.s("pr_url")) || has(t.s("no_pr")) || landed(t).is_some() || !crate::projects::task_ships_pr(app, t)? {
        return Ok(());
    }
    let who: Vec<String> = waitsfor::waiting_on(app, t)?.iter().filter(|w| waitsfor::merged_ids(w).contains(&t.id())).map(|w| rf("task", w.id())).collect();
    if who.is_empty() {
        return Ok(());
    }
    let me = rf("task", t.id());
    err(
        409,
        format!(
            "{} for {me}'s PR to merge, but {me} has no PR linked, so that merge would never come. \
             Link its PR (tb submit --pr-body <file>, or --pr <link>), or say why there's none (tb submit --no-pr \"<why>\").",
            who.join(", ") + if who.len() == 1 { " waits" } else { " wait" }
        ),
    )
}

fn text(t: &Row, held: &[Row]) -> String {
    let me = rf("task", t.id());
    let who: Vec<String> = held.iter().map(|w| rf("task", w.id())).collect();
    let (who, verb) = match who.as_slice() {
        [one] => (one.clone(), "waits"),
        _ => (who.join(", "), "wait"),
    };
    let drop = format!("To let a waiter go on without it: tb wait-for --drop {me} --task {}.", rf("task", held[0].id()));
    match t.i("pr_num") {
        Some(n) => format!("{me}'s PR #{n} was declined, so {who} {verb} for work that won't merge. If it landed another way: tb task set {me} --landed <commit>. {drop}"),
        None => format!(
            "{me} finished with no PR linked, so {who} {verb} for a merge that won't come. If its work went through another PR: tb task set {me} --pr-link <link>. \
             If it landed another way: tb task set {me} --landed <commit>. {drop}"
        ),
    }
}

/// Raises, updates or clears the Needs-you for `t`'s held waiters. Whether one is up now.
pub fn check(app: &App, t: &Row) -> Result<bool> {
    let held = if stuck(t) { held(app, t)? } else { vec![] };
    let k = key(t.id());
    if held.is_empty() {
        dispatch::clear_alert_key(app, &k)?;
        return Ok(false);
    }
    let text = text(t, &held);
    let up = dispatch::alerts(app).into_iter().find(|a| a["key"] == k.as_str());
    if up.as_ref().is_some_and(|a| a["text"] == text.as_str()) {
        return Ok(true);
    }
    if up.is_some() {
        dispatch::clear_alert_key(app, &k)?;
    }
    dispatch::add_alert_keyed(app, &text, Some(t.id()), t.i("goal_id"), &k)?;
    Ok(true)
}

/// The runner's sweep: each done task with a declined PR or no PR that a task still waits for is checked,
/// and an alert whose task no longer holds anyone is cleared.
pub fn sweep(app: &App) -> Result<()> {
    let rows = app.db.q(
        "SELECT * FROM tasks WHERE status = 'done' AND (landed IS NULL OR landed = '') \
         AND ((pr_num IS NOT NULL AND (pr_phase = 'declined' OR UPPER(pr_state) IN ('CLOSED', 'DECLINED', 'SUPERSEDED'))) \
         OR (pr_num IS NULL AND failed = 0 AND COALESCE(pr_url, '') = '' AND COALESCE(no_pr, '') = '')) \
         AND EXISTS (SELECT 1 FROM tasks w WHERE w.status != 'done' AND w.waits_for IS NOT NULL \
         AND EXISTS (SELECT 1 FROM json_each(w.waits_for) WHERE value = tasks.id))",
        p![],
    )?;
    let mut up: Vec<String> = vec![];
    for t in &rows {
        if check(app, t)? {
            up.push(key(t.id()));
        }
    }
    let stale: Vec<String> = dispatch::alerts(app)
        .into_iter()
        .filter_map(|a| a["key"].as_str().map(|k| k.to_string()))
        .filter(|k| k.starts_with(PREFIX) && !up.contains(k))
        .collect();
    for k in stale {
        dispatch::clear_alert_key(app, &k)?;
    }
    Ok(())
}

/// The commit `given` names in the task's repository, or `given` itself when it's a commit id git can't
/// find there (not fetched yet).
fn resolve(app: &App, t: &Row, given: &str) -> Result<String> {
    let hex = (7..=40).contains(&given.len()) && given.chars().all(|c| c.is_ascii_hexdigit());
    if let Some(repo) = repo(app, t)? {
        let g = git(&repo, &["rev-parse", "--verify", "--quiet", &format!("{given}^{{commit}}")], GIT_SECS);
        if g.code == 0 && !g.out.trim().is_empty() {
            return Ok(g.out.trim().to_string());
        }
    }
    if hex {
        return Ok(given.to_lowercase());
    }
    err(400, format!("{given} isn't a commit: give the commit id its work landed in on the main branch."))
}

fn repo(app: &App, t: &Row) -> Result<Option<String>> {
    Ok(crate::worktrees::repo_of(app, t)?.filter(|r| std::path::Path::new(r).is_dir()))
}

/// `tb task set T<n> --pr-link <link>`: a done task's work went through this PR. It joins the task that
/// owns the PR, or links it when none does, so tasks waiting for its merge follow that PR. Whether anything
/// changed.
pub fn set_pr_link(app: &App, t: &Row, value: &Value, who: &str) -> Result<bool> {
    let Some(pr) = value.as_str().and_then(find_pr) else { return err(400, "That isn't a pull request link (GitHub, GitLab or Bitbucket).") };
    let me = rf("task", t.id());
    if t.s("pr_url") == Some(pr.url.as_str()) {
        return Ok(false);
    }
    if !waitsfor::ready(Some(t)) {
        return err(409, format!("{me} isn't done, so it links its PR when it finishes: tb submit --pr <link>."));
    }
    if has(t.s("pr_url")) {
        return err(409, format!("{me} has a PR already."));
    }
    if has(t.s("no_pr")) {
        return err(409, format!("{me} finished without a PR on purpose, so its waiters aren't held by it."));
    }
    crate::prflow::refuse_other_repo(app, t, &pr)?;
    if crate::samepr::owner(app, t, &pr)?.is_some() {
        if let Some(why) = crate::samepr::cant_join(app, t)? {
            return err(409, why);
        }
    }
    let linked = crate::samepr::join_owner(app, t, &pr, who)? || crate::prflow::link_pr(app, t, &pr, who)?;
    check(app, &board::get_task(app, t.id())?)?;
    Ok(linked)
}

/// `tb task set T<n> --landed <commit>`: its work landed on the main branch in `<commit>`, outside its
/// PR, so tasks waiting for its merge go on; `none` takes that back. Whether anything changed.
pub fn set_landed(app: &App, t: &Row, value: &Value, who: &str) -> Result<bool> {
    let given = match value {
        Value::Null => String::new(),
        Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    };
    let me = rf("task", t.id());
    if given.is_empty() || given.eq_ignore_ascii_case("none") {
        if landed(t).is_none() {
            return Ok(false);
        }
        board::update_task(app, t.id(), fields!["landed" => null])?;
        board::log_event(app, t.id(), who, "note", "No longer marked as landed on the main branch")?;
        check(app, &board::get_task(app, t.id())?)?;
        return Ok(true);
    }
    if !waitsfor::ready(Some(t)) {
        return err(409, format!("{me} isn't done, so its work can't have landed yet."));
    }
    if waitsfor::on_main(app, t)? && landed(t).is_none() {
        return err(409, format!("{me}'s PR #{} merged, so its work is already on the main branch.", t.i0("pr_num")));
    }
    let sha = resolve(app, t, &given)?;
    if landed(t) == Some(sha.as_str()) {
        return Ok(false);
    }
    board::update_task(app, t.id(), fields!["landed" => sha.clone()])?;
    let short = &sha[..sha.len().min(12)];
    let line = match t.i("pr_num") {
        Some(n) => format!("Its work landed on the main branch in {short}, outside PR #{n}"),
        None => format!("Its work landed on the main branch in {short}"),
    };
    board::log_event(app, t.id(), who, "note", &line)?;
    check(app, &board::get_task(app, t.id())?)?;
    Ok(true)
}
