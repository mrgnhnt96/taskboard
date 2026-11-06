//! The home page (`GET /home`): the goals with work in flight, each with the tasks of the wave it's
//! on and the ones still open from earlier waves, the tasks outside any goal, and the alerts as the
//! "Needs you" list, each with its task's card.
//!
//! In flight: working, needs you, starting, or done with its PR still open. A goal's wave is the
//! latest one with a task in flight, else the first with a task queued; a goal without waves is one
//! wave. Planned tasks never show: nothing runs them until the owner starts the goal.
//!
//! Put off, an item leaves `needs` for `later` until its time comes. Each item says what it `holds`
//! up, and `waiting` counts the agents waiting on the owner, put off or not. A goal stopped after a
//! wave is a Needs you item too: its entry's `stage`, under a `stage:G12:1` id. Nothing from a paused
//! or set-aside goal is listed, put off or not.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::app::App;
use crate::dispatch::{self, Later};
use crate::util::*;
use crate::{board, human, p, quiet, waitsfor, waves};

fn in_flight(t: &Row) -> bool {
    match t.s("status") {
        Some("working") | Some("needs") => true,
        Some("queued") => t.i("start_job").is_some(),
        Some("done") => !t.b("failed") && board::pr_still_open(t),
        _ => false,
    }
}

fn waiting(t: &Row) -> bool {
    t.s("status") == Some("queued") && t.i("start_job").is_none()
}

/// The wave a goal is on: the latest with a task in flight, else the first with one queued.
fn current_wave(tasks: &[Row]) -> Option<i64> {
    tasks.iter().filter(|t| in_flight(t)).filter_map(|t| t.i("wave")).max().or_else(|| tasks.iter().filter(|t| waiting(t)).filter_map(|t| t.i("wave")).min())
}

/// A task without a wave in a goal with waves runs with whichever wave is on.
fn in_wave(t: &Row, wave: Option<i64>) -> bool {
    wave.is_none() || t.i("wave").is_none_or(|w| w == wave.unwrap_or(w))
}

fn cards(app: &App, rows: &[&Row]) -> Result<Vec<Value>> {
    rows.iter().map(|t| board::task_card(app, t)).collect()
}

/// One goal's entry, or `None` when nothing in it is in flight or queued.
fn goal_entry(app: &App, g: &Row) -> Result<Option<Value>> {
    let tasks = board::goal_tasks(app, g.id())?;
    let wave = if crate::waves::uses_waves(&tasks) { current_wave(&tasks) } else { None };
    let now: Vec<&Row> = tasks.iter().filter(|t| in_flight(t) && in_wave(t, wave)).collect();
    let earlier = |t: &Row| matches!((t.i("wave"), wave), (Some(w), Some(cur)) if w < cur);
    let left: Vec<&Row> = tasks.iter().filter(|t| earlier(t) && (in_flight(t) || waiting(t))).collect();
    let queued = tasks.iter().filter(|t| waiting(t) && in_wave(t, wave)).count();
    if now.is_empty() && left.is_empty() && queued == 0 {
        return Ok(None);
    }
    let mut left_waves: Vec<i64> = left.iter().filter_map(|t| t.i("wave")).collect();
    left_waves.sort();
    left_waves.dedup();
    let mut waves: Vec<i64> = tasks.iter().filter_map(|t| t.i("wave")).collect();
    waves.sort();
    waves.dedup();
    let mut goal = board::goal_dict(app, g)?;
    let stage = stage_entry(app, g, &tasks, &mut goal)?;
    Ok(Some(json!({
        "goal": goal,
        "stage": stage,
        "project": g.v("project"),
        "wave": wave,
        "waves": waves.len(),
        "now": cards(app, &now)?,
        "left": cards(app, &left)?,
        "left_waves": left_waves,
        "queued": queued,
    })))
}

/// Tasks outside any goal, one entry per project.
fn loose_entries(app: &App, project: &str) -> Result<Vec<Value>> {
    let rows = app.db.q("SELECT * FROM tasks WHERE goal_id IS NULL ORDER BY project, id", p![])?;
    let mut out: Vec<Value> = vec![];
    let mut projects: Vec<String> = rows.iter().filter_map(|t| t.s("project").map(str::to_string)).collect();
    projects.dedup();
    for name in projects.into_iter().filter(|n| project == "all" || n == project) {
        let mine: Vec<&Row> = rows.iter().filter(|t| t.s("project") == Some(name.as_str())).collect();
        let now: Vec<&Row> = mine.iter().copied().filter(|t| in_flight(t)).collect();
        let queued = mine.iter().filter(|t| waiting(t)).count();
        if now.is_empty() && queued == 0 {
            continue;
        }
        out.push(json!({"goal": null, "project": name, "wave": null, "waves": 0, "now": cards(app, &now)?, "left": [], "left_waves": [], "queued": queued}));
    }
    Ok(out)
}

/// Needs you before working before the rest; newest goal first within each.
fn rank(e: &Value) -> u8 {
    let now = e["now"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if now.iter().any(|c| c["status"] == "needs") {
        0
    } else if now.iter().any(|c| c["status"] == "working") {
        1
    } else {
        2
    }
}

/// The stage the goal is stopped at for the owner's review, unless it's put off: then the goal reads
/// as not stopped.
fn stage_entry(app: &App, g: &Row, tasks: &[Row], goal: &mut Value) -> Result<Value> {
    if goal["stopped"].is_null() || g.b("paused") || !waves::uses_waves(tasks) {
        return Ok(Value::Null);
    }
    let Some(wave) = waves::stopped_wave(&waves::waves(app, g, tasks)?) else { return Ok(Value::Null) };
    let id = stage_id(g.id(), wave);
    if put_away_list(app).get(&id).is_some_and(dispatch::snoozed) {
        goal["stopped"] = Value::Null;
        return Ok(Value::Null);
    }
    Ok(json!({"id": id, "wave": wave, "holds": stage_holds(g.id(), tasks)}))
}

/// A stage holds up the rest of its goal.
fn stage_holds(goal: i64, tasks: &[Row]) -> Value {
    let queued: Vec<String> = tasks.iter().filter(|t| t.s("status") == Some("queued")).map(human::task_ref).collect();
    json!({"tasks": queued, "goal": rf("goal", goal)})
}

struct GoalView {
    goal: Row,
    tasks: Vec<Row>,
    waves: Vec<Value>,
}

/// The goals the items belong to, each read once per page.
#[derive(Default)]
struct Goals(HashMap<i64, Option<GoalView>>);

impl Goals {
    fn get(&mut self, app: &App, id: i64) -> Result<Option<&GoalView>> {
        if !self.0.contains_key(&id) {
            let view = match board::find_goal(app, Some(id))? {
                Some(goal) => {
                    let tasks = board::goal_tasks(app, id)?;
                    let waves = if waves::uses_waves(&tasks) { waves::waves(app, &goal, &tasks)? } else { vec![] };
                    Some(GoalView { goal, tasks, waves })
                }
                None => None,
            };
            self.0.insert(id, view);
        }
        Ok(self.0[&id].as_ref())
    }
}

/// The queued tasks in `t`'s goal that its goal's order holds until `t` is done: the later waves, once
/// `t`'s wave has nothing left to start, or the tasks after it in a goal that runs in order.
fn held_in_goal(v: &GoalView, t: &Row) -> Vec<String> {
    let queued = |x: &&Row| x.s("status") == Some("queued") && x.id() != t.id();
    if let (false, Some(wave)) = (v.waves.is_empty(), t.i("wave")) {
        let own = v.waves.iter().find(|w| w["wave"].as_i64() == Some(wave));
        if own.is_none_or(|w| w["to_start"].as_array().is_some_and(|a| !a.is_empty())) {
            return vec![];
        }
        let held_here = |x: &&Row| x.i("wave").is_some_and(|n| n > wave) && waves::holding_wave(x, &v.waves).is_some_and(|h| h["wave"].as_i64() == Some(wave));
        return v.tasks.iter().filter(queued).filter(held_here).map(human::task_ref).collect();
    }
    if v.waves.is_empty() && v.goal.b("run_in_order") {
        return v.tasks.iter().skip_while(|x| x.id() != t.id()).filter(queued).map(human::task_ref).collect();
    }
    vec![]
}

/// What an open task holds up: the tasks that wait for it and its goal. A to-do the owner said not
/// to wait for holds up nothing.
fn task_holds(app: &App, goals: &mut Goals, t: &Row) -> Result<Value> {
    if t.s("status") == Some("done") || human::released(t) {
        return Ok(no_holds());
    }
    let mut tasks: Vec<String> = waitsfor::waiting_on(app, t)?.iter().map(human::task_ref).collect();
    if let Some(view) = t.i("goal_id").map(|g| goals.get(app, g)).transpose()?.flatten() {
        for r in held_in_goal(view, t) {
            if !tasks.contains(&r) {
                tasks.push(r);
            }
        }
    }
    Ok(json!({"tasks": tasks, "goal": t.i("goal_id").map(|g| rf("goal", g))}))
}

fn no_holds() -> Value {
    json!({"tasks": [], "goal": null})
}

/// An agent is waiting on the owner's word. A lost terminal or a to-do isn't that.
fn agent_waiting(t: &Row) -> bool {
    t.s("status") == Some("needs") && !t.b("lost") && matches!(t.s("needs_reason"), Some("question") | Some("attention"))
}

/// Whether alert `a` belongs under Agents waiting: its agent waits on the owner's word, or has gone quiet.
pub fn alert_waits(app: &App, a: &Value) -> bool {
    gone_quiet(a) || a["task"].as_str().and_then(number).and_then(|id| board::get_task(app, id).ok()).is_some_and(|t| agent_waiting(&t))
}

fn gone_quiet(a: &Value) -> bool {
    a["key"].as_str().is_some_and(quiet::is_key)
}

/// The agents waiting on the owner in goals that are running, whether or not the item is put off.
/// Work gone quiet counts too: its agent has stopped and only the owner can tell it why.
pub fn waiting_count(app: &App) -> Result<i64> {
    let n = app.db.val(
        "SELECT COUNT(*) FROM tasks t LEFT JOIN goals g ON g.id = t.goal_id WHERE t.status = 'needs' \
         AND t.needs_reason IN ('question', 'attention') AND COALESCE(t.lost, 0) = 0 \
         AND COALESCE(g.paused, 0) = 0 AND COALESCE(g.deprioritized, 0) = 0",
        p![],
    )?;
    let resting = resting_goals(app)?;
    let quiet = dispatch::listing(app).iter().filter(|a| gone_quiet(a) && !rests(app, a, &resting)).count() as i64;
    Ok(n.as_i64().unwrap_or(0) + quiet)
}

/// An alert as a Needs you item: with its task's card (or null), its goal's name and what it holds up.
fn alert_item(app: &App, goals: &mut Goals, mut a: Value) -> Result<Value> {
    let task = a["task"].as_str().and_then(number).and_then(|id| board::get_task(app, id).ok());
    let card = task.as_ref().map(|t| board::task_card(app, t)).transpose()?.unwrap_or(Value::Null);
    let goal_name = match a["goal"].as_str().and_then(number) {
        Some(id) => app.db.val("SELECT name FROM goals WHERE id = ?", p![id])?,
        None => card["goal"]["name"].clone(),
    };
    let holds = match &task {
        _ if a["key"].as_str().is_some_and(|k| k.starts_with("miss:")) => json!({"tasks": [], "goal": a["goal"]}),
        Some(t) => task_holds(app, goals, t)?,
        None => no_holds(),
    };
    a["card"] = card;
    a["goal_name"] = goal_name;
    a["waiting"] = json!(gone_quiet(&a) || task.as_ref().is_some_and(agent_waiting));
    a["holds"] = holds;
    Ok(a)
}

/// A task that needs you with no alert up, under a `task:T<n>` id.
fn task_item(app: &App, goals: &mut Goals, t: &Row, card: Value) -> Result<Value> {
    let text = card["question"].as_str().or(card["latest"].as_str()).unwrap_or("").to_string();
    let r = rf("task", t.id());
    Ok(json!({"id": format!("{TASK_ID}{r}"), "at": t.v("updated_at"), "text": text, "task": r, "goal": card["goal"]["ref"],
              "goal_name": card["goal"]["name"], "card": card, "waiting": agent_waiting(t), "holds": task_holds(app, goals, t)?}))
}

/// When an item put off comes back. `a` is an alert or a put-away entry.
fn later_of(a: &Value) -> Value {
    json!({"until": a["snoozed_until"], "back": a["snoozed_back"].is_string(), "after_goal": a["snoozed_goal"].as_i64().map(|g| rf("goal", g)),
           "waiting_on": a["snoozed_waiting"]})
}

/// The stages put off that are still stopped.
fn later_stages(app: &App, goals: &mut Goals, away: &Row, resting: &HashSet<i64>) -> Result<Vec<Value>> {
    let mut out = vec![];
    for (id, entry) in away.iter().filter(|(_, p)| dispatch::snoozed(p)) {
        let Some((g, wave)) = stage_parts(id).filter(|(g, _)| !resting.contains(g)) else { continue };
        let Some(v) = goals.get(app, g)? else { continue };
        if v.goal.b("archived") || waves::stopped_wave(&v.waves) != Some(wave) {
            continue;
        }
        out.push(json!({"id": id, "text": waves::stopped_line(&v.waves), "goal": rf("goal", g), "goal_name": v.goal.v("name"), "stage": true,
                        "wave": wave, "waiting": false, "holds": stage_holds(g, &v.tasks), "later": later_of(entry)}));
    }
    Ok(out)
}

/// The goals resting from Needs you: paused or set aside. Their closed tasks stay on the goal's
/// page, and Play there restarts them.
fn resting_goals(app: &App) -> Result<HashSet<i64>> {
    Ok(app.db.q("SELECT id FROM goals WHERE paused = 1 OR deprioritized = 1", p![])?.iter().map(|g| g.id()).collect())
}

fn alert_goal(app: &App, a: &Value) -> Option<i64> {
    let id = |k: &str| a[k].as_str().and_then(number);
    id("goal").or_else(|| board::get_task(app, id("task")?).ok()?.i("goal_id"))
}

/// An alert in a resting goal stays off the list. An urgent one doesn't: it keeps repeating, and
/// the list is where it's answered.
fn rests(app: &App, a: &Value, resting: &HashSet<i64>) -> bool {
    a["urgent"] != true && alert_goal(app, a).is_some_and(|g| resting.contains(&g))
}

/// Whether alert `a` is off Needs you for its goal, so nothing should repeat it either.
pub fn alert_rests(app: &App, a: &Value) -> bool {
    resting_goals(app).is_ok_and(|r| rests(app, a, &r))
}

/// The Needs you list and what's put off. An alert snoozed takes its task with it until it ends.
/// Nothing from a resting goal is on either.
fn needs(app: &App) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut goals = Goals::default();
    let (mut out, mut later) = (vec![], vec![]);
    let resting = resting_goals(app)?;
    let (snoozed, up): (Vec<Value>, Vec<Value>) = dispatch::listing(app).into_iter().partition(dispatch::snoozed);
    for a in up.into_iter().filter(|a| !rests(app, a, &resting)) {
        out.push(alert_item(app, &mut goals, a)?);
    }
    for a in snoozed.iter().filter(|a| !rests(app, a, &resting)) {
        let mut item = alert_item(app, &mut goals, a.clone())?;
        item["later"] = later_of(a);
        later.push(item);
    }
    // A task that needs you with no alert up (dismissed, or never raised) still does, under a
    // `task:T<n>` id that Dismiss and Snooze put away (`put_away`).
    let away = put_away_list(app);
    let mut stopped: Vec<StoppedGoal> = vec![];
    for t in app.db.q("SELECT * FROM tasks WHERE status = 'needs' ORDER BY updated_at DESC", p![])? {
        let r = rf("task", t.id());
        if t.i("goal_id").is_some_and(|g| resting.contains(&g)) {
            continue;
        }
        if out.iter().chain(&snoozed).any(|a| a["task"].as_str() == Some(r.as_str())) {
            continue;
        }
        let put = away.get(&r);
        if let Some(p) = put.filter(|p| dispatch::snoozed(p)) {
            let mut item = task_item(app, &mut goals, &t, board::task_card(app, &t)?)?;
            item["later"] = later_of(p);
            later.push(item);
        } else if !put.is_some_and(|p| p["asking"].as_str() == Some(asking(&t).as_str())) {
            let card = board::task_card(app, &t)?;
            match t.i("goal_id").filter(|_| quietly_stopped(&card)) {
                Some(goal) => {
                    let holds = task_holds(app, &mut goals, &t)?;
                    let at = stopped.iter().position(|s| s.goal == goal).unwrap_or_else(|| {
                        stopped.push(StoppedGoal { goal, cards: vec![], holds: vec![] });
                        stopped.len() - 1
                    });
                    stopped[at].add(card, &holds);
                }
                None => out.push(task_item(app, &mut goals, &t, card)?),
            }
        }
    }
    out.extend(stopped.iter().map(stopped_row));
    later.extend(later_stages(app, &mut goals, &away, &resting)?);
    Ok((out, later))
}

/// The number in `T12` or `G3`.
pub(crate) fn number(r: &str) -> Option<i64> {
    r.trim_start_matches(['T', 'G']).parse().ok()
}

/// A terminal closed by hand on work that doesn't look finished: quiet, and restarted with its goal.
fn quietly_stopped(card: &Value) -> bool {
    card["closed_by_hand"] == true && card["looks_finished"].is_null()
}

/// One goal on the Stopped list while it's being built: its closed tasks' cards, newest close first,
/// and what they hold up.
struct StoppedGoal {
    goal: i64,
    cards: Vec<Value>,
    holds: Vec<String>,
}

impl StoppedGoal {
    fn add(&mut self, card: Value, holds: &Value) {
        self.cards.push(card);
        for r in refs(&holds["tasks"]) {
            if !self.holds.contains(&r) {
                self.holds.push(r);
            }
        }
    }
}

fn refs(list: &Value) -> Vec<String> {
    list.as_array().map(|a| a.iter().filter_map(|r| r.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// One goal's line under Stopped: `stopped` holds the tasks its Restart starts again
/// (`stopped_tasks`). It has no `task`, so Dismiss and Snooze don't reach it.
fn stopped_row(StoppedGoal { goal, cards, holds }: &StoppedGoal) -> Value {
    let first = &cards[0];
    let tasks: Vec<&Value> = cards.iter().map(|c| &c["ref"]).collect();
    let text = cards.iter().map(|c| format!("{} {}", c["ref"].as_str().unwrap_or(""), c["title"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n");
    let goal_ref = rf("goal", *goal);
    let name = first["goal"]["name"].as_str().unwrap_or("");
    json!({"id": format!("{STOPPED_ID}{goal_ref}"), "at": first["updated_at"], "title": format!("{name} · {} closed", tasks.len()), "text": text,
           "goal": goal_ref, "goal_name": name, "project": first["project"], "card": null, "waiting": false,
           "holds": {"tasks": holds, "goal": goal_ref}, "stopped": {"tasks": tasks}})
}

/// The tasks on goal `id`'s Stopped line, the ones its Restart starts again. A goal at rest has no
/// line: Play on it restarts them.
pub fn stopped_tasks(app: &App, id: i64) -> Result<Vec<Row>> {
    if resting_goals(app)?.contains(&id) {
        return Ok(vec![]);
    }
    let alerted: Vec<Value> = dispatch::listing(app);
    let away = put_away_list(app);
    let mut out = vec![];
    for t in board::goal_tasks(app, id)? {
        let r = rf("task", t.id());
        let alone = !alerted.iter().any(|a| a["task"].as_str() == Some(r.as_str())) && !is_put_away(&away, &t);
        if alone && board::closed_by_hand(app, &t)? && crate::lost_done::card(&t).is_null() {
            out.push(t);
        }
    }
    Ok(out)
}

/// The id prefix of a Needs you item that's a task with no alert up: `task:T12`.
pub const TASK_ID: &str = "task:";
/// The id prefix of a goal's line under Stopped: `stopped:G12`.
const STOPPED_ID: &str = "stopped:";

/// The id prefix of a goal's finished stage: `stage:G12:1`, stopped after wave 1.
pub const STAGE_ID: &str = "stage:";

fn stage_id(goal: i64, wave: i64) -> String {
    format!("{STAGE_ID}{}:{wave}", rf("goal", goal))
}

fn stage_parts(id: &str) -> Option<(i64, i64)> {
    let (goal, wave) = id.strip_prefix(STAGE_ID)?.split_once(':')?;
    Some((number(goal)?, wave.parse().ok()?))
}

/// A Needs you item that has no alert behind it.
pub fn not_an_alert(id: &str) -> bool {
    id.starts_with(TASK_ID) || id.starts_with(STAGE_ID)
}

/// The items put away from Needs you while no alert is up: tasks by ref, stages by id. A task is
/// dismissed (`asking`, what it asked then) or snoozed, and a stage snoozed, as an alert's (`Later`).
const PUT_AWAY: &str = "needs_put_away";

fn put_away_list(app: &App) -> Row {
    jloads_obj(crate::reqcache::setting(app, PUT_AWAY).ok().flatten().as_deref())
}

/// Whether a task with no alert up was put away and still is: dismissed while asking the same, or
/// in a snooze.
fn is_put_away(away: &Row, t: &Row) -> bool {
    away.get(&rf("task", t.id())).is_some_and(|p| p["asking"].as_str() == Some(asking(t).as_str()) || dispatch::snoozed(p))
}

/// What a task that needs you is asking: a new question or a new reason brings a dismissed one back.
fn asking(t: &Row) -> String {
    format!("{}|{}|{}", t.st("needs_reason"), t.b("lost"), t.st("question"))
}

fn save_put_away(app: &App, away: Row) -> Result<()> {
    crate::reqcache::forget(&format!("setting:{PUT_AWAY}"));
    app.db.set_setting(PUT_AWAY, Some(&jdumps(&Value::Object(away))))
}

fn later_entry(later: &Later) -> Value {
    let mut entry = json!({});
    later.mark(&mut entry);
    entry
}

/// The put-away entry for a stage, if the goal is stopped there.
fn stage_entry_for(app: &App, id: &str, later: Option<Later>) -> Result<Value> {
    let Some((g, wave)) = stage_parts(id) else { return err(404, "There's no such stage.") };
    let Some(later) = later else { return err(409, "A finished stage stays until it's continued or put off.") };
    let Some(goal) = board::find_goal(app, Some(g))? else { return err(404, "There's no such goal.") };
    let tasks = board::goal_tasks(app, g)?;
    if waves::stopped_wave(&waves::waves(app, &goal, &tasks)?) != Some(wave) {
        return err(404, "That goal isn't stopped at that stage now.");
    }
    Ok(later_entry(&later))
}

/// Dismiss (`later` None) or Snooze (`Some`) on a Needs you item that isn't an alert: a `task:T<n>`
/// with no alert up, or a finished `stage:G<n>:<wave>`. Returns what was kept, for the app's toast.
pub fn put_away(app: &App, id: &str, later: Option<Later>) -> Result<Value> {
    let (key, entry) = if id.starts_with(STAGE_ID) {
        (id.to_string(), stage_entry_for(app, id, later)?)
    } else {
        let t = id.strip_prefix(TASK_ID).and_then(number).map(|n| board::get_task(app, n));
        let Some(Ok(t)) = t.filter(|t| t.as_ref().is_ok_and(|t| t.s("status") == Some("needs"))) else {
            return err(404, "That task doesn't need you now.");
        };
        (rf("task", t.id()), later.map(|l| later_entry(&l)).unwrap_or_else(|| json!({"asking": asking(&t)})))
    };
    let needs: Vec<String> = app.db.q("SELECT id FROM tasks WHERE status = 'needs'", p![])?.iter().map(|t| rf("task", t.id())).collect();
    let mut away = put_away_list(app);
    away.retain(|r, p| needs.contains(r) || (r.starts_with(STAGE_ID) && dispatch::snoozed(p)));
    away.insert(key, entry.clone());
    save_put_away(app, away)?;
    Ok(entry)
}

/// Drops the put-away entries waiting for a goal that's over.
pub fn bring_back_after_goals(app: &App) -> Result<()> {
    let mut away = put_away_list(app);
    let mut over = vec![];
    for (r, p) in &away {
        if let Some(g) = p["snoozed_goal"].as_i64() {
            if dispatch::goal_over(app, g)? {
                over.push(r.clone());
            }
        }
    }
    if over.is_empty() {
        return Ok(());
    }
    away.retain(|r, _| !over.contains(r));
    save_put_away(app, away)
}

/// The goal a Needs you item belongs to, if any.
pub fn item_goal(app: &App, id: &str) -> Result<Option<i64>> {
    if let Some((g, _)) = stage_parts(id) {
        return Ok(Some(g));
    }
    let of_task = |n: i64| Ok(board::find_task(app, Some(n))?.and_then(|t| t.i("goal_id")));
    if let Some(n) = id.strip_prefix(TASK_ID).and_then(number) {
        return of_task(n);
    }
    let alerts = dispatch::alerts(app);
    let Some(a) = alerts.iter().find(|a| a["id"] == id) else { return Ok(None) };
    match (a["goal"].as_str().and_then(number), a["task_id"].as_i64()) {
        (Some(g), _) => Ok(Some(g)),
        (None, Some(n)) => of_task(n),
        _ => Ok(None),
    }
}

pub fn page(app: &App, project: &str) -> Result<Value> {
    let mut entries = vec![];
    for g in app.db.q("SELECT * FROM goals ORDER BY id DESC", p![])? {
        if g.b("archived") || g.b("deprioritized") || (project != "all" && g.s("project") != Some(project)) {
            continue;
        }
        if let Some(e) = goal_entry(app, &g)? {
            entries.push(e);
        }
    }
    entries.sort_by_key(rank);
    entries.extend(loose_entries(app, project)?);
    entries.sort_by(|a, b| a["project"].as_str().unwrap_or("").cmp(b["project"].as_str().unwrap_or("")));
    let (needs, later) = needs(app)?;
    let next_back = later.iter().filter_map(|i| i["later"]["until"].as_str()).min().map(str::to_string);
    Ok(json!({"goals": entries, "needs": needs, "later_count": later.len(), "next_back": next_back, "later": later, "waiting": waiting_count(app)?}))
}
