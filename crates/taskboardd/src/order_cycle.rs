//! Waits that cycle with a goal's run-in-order order. In a goal that runs in order, a task can't start
//! until the tasks before it are done, so an earlier task that waits for a later one (or for anything
//! that comes to wait on it) holds both forever, and the rest of the goal behind them. A wait that would
//! make such a cycle is refused (`refusal`), and one already on the board (the order changed, or the
//! goal was set to run in order after the wait) goes on Needs you until it's broken (`sweep`).

use std::collections::{HashMap, VecDeque};

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;
use crate::waitsfor;

const PREFIX: &str = "ordercycle:";
const RAISED: &str = "order_cycles_raised";

/// One step of a cycle: `from` can't start until `to` is done, because it waits for it or because it
/// comes after it in goal `in_goal`'s order.
#[derive(Clone, Debug)]
struct Hop {
    from: i64,
    to: i64,
    in_goal: Option<i64>,
}

/// Which waits count. `Plan` asks whether a wait would ever cycle, so a planned task counts as the run
/// it'll join; `Live` asks what holds the queued tasks right now, as the runner sees it.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Plan,
    Live,
}

struct Walk<'a> {
    app: &'a App,
    mode: Mode,
    tasks: HashMap<i64, Option<Row>>,
    order: HashMap<i64, Vec<Row>>,
}

impl<'a> Walk<'a> {
    fn new(app: &'a App, mode: Mode) -> Self {
        Walk { app, mode, tasks: HashMap::new(), order: HashMap::new() }
    }

    fn task(&mut self, id: i64) -> Result<Option<Row>> {
        if let Some(t) = self.tasks.get(&id) {
            return Ok(t.clone());
        }
        let t = board::find_task(self.app, Some(id))?;
        self.tasks.insert(id, t.clone());
        Ok(t)
    }

    /// The goal's tasks in order when it runs in order without waves (waves take the place of the order),
    /// else none.
    fn in_order(&mut self, goal: i64) -> Result<&[Row]> {
        if !self.order.contains_key(&goal) {
            let g = board::find_goal(self.app, Some(goal))?;
            let rows = match g.filter(|g| g.b("run_in_order")) {
                Some(g) => {
                    let rows = board::goal_tasks(self.app, g.id())?;
                    if crate::waves::uses_waves(&rows) { vec![] } else { rows }
                }
                None => vec![],
            };
            self.order.insert(goal, rows);
        }
        Ok(&self.order[&goal])
    }

    /// Whether `x` can be held back at all: a queued task, or in `Plan` a planned one too.
    fn waits(&self, x: &Row) -> bool {
        match x.s("status") {
            Some("queued") => true,
            Some("planned") => self.mode == Mode::Plan,
            _ => false,
        }
    }

    /// What holds `x` back: the tasks it waits for that aren't ready, and the earlier tasks in its goal's
    /// order that aren't done.
    fn holds(&mut self, x: &Row) -> Result<Vec<Hop>> {
        let mut out = vec![];
        if x.s("status") == Some("done") || crate::human::released(x) {
            return Ok(out);
        }
        for d in waitsfor::deps(x) {
            let other = self.task(d)?;
            let held = match self.mode {
                Mode::Plan => !waitsfor::ready(other.as_ref()),
                Mode::Live => self.waits(x) && other.is_some() && !waitsfor::ready_for(self.app, x, d, other.as_ref())?,
            };
            if held {
                out.push(Hop { from: x.id(), to: d, in_goal: None });
            }
        }
        let Some(g) = x.i("goal_id").filter(|_| self.waits(x)) else { return Ok(out) };
        let mode = self.mode;
        for y in self.in_order(g)? {
            if y.id() == x.id() {
                break;
            }
            let pending = match y.s("status") {
                Some("done") => false,
                Some("planned") => mode == Mode::Plan,
                _ => true,
            };
            if pending && !crate::human::released(y) {
                out.push(Hop { from: x.id(), to: y.id(), in_goal: Some(g) });
            }
        }
        Ok(out)
    }

    /// The steps from `start` to `target`, when `start` can't start until `target` is done.
    fn path(&mut self, start: i64, target: i64) -> Result<Option<Vec<Hop>>> {
        let mut came: HashMap<i64, Hop> = HashMap::new();
        let mut todo = VecDeque::from([start]);
        let mut seen = vec![start];
        while let Some(id) = todo.pop_front() {
            if id == target {
                let mut hops = vec![];
                let mut at = target;
                while at != start {
                    let h = came[&at].clone();
                    at = h.from;
                    hops.push(h);
                }
                hops.reverse();
                return Ok(Some(hops));
            }
            if seen.len() > 500 {
                break;
            }
            let Some(x) = self.task(id)? else { continue };
            for h in self.holds(&x)? {
                if !seen.contains(&h.to) {
                    seen.push(h.to);
                    todo.push_back(h.to);
                    came.insert(h.to, h);
                }
            }
        }
        Ok(None)
    }

    fn name(&mut self, id: i64) -> Result<String> {
        Ok(self.task(id)?.map(|t| crate::human::task_ref(&t)).unwrap_or_else(|| rf("task", id)))
    }

    /// "T6 runs after T2 in G4's order, T2 waits for T6".
    fn say(&mut self, hops: &[Hop]) -> Result<String> {
        let mut bits = vec![];
        for h in hops {
            let (from, to) = (self.name(h.from)?, self.name(h.to)?);
            bits.push(match h.in_goal {
                Some(g) => format!("{from} runs after {to} in {}'s order", rf("goal", g)),
                None => format!("{from} waits for {to}"),
            });
        }
        Ok(bits.join(", "))
    }

    /// How to break the cycle: move the first pair the order holds, or let that goal run out of order.
    fn fix(&mut self, app: &App, hops: &[Hop]) -> Result<String> {
        let Some(h) = hops.iter().find(|h| h.in_goal.is_some()) else { return Ok(String::new()) };
        let g = rf("goal", h.in_goal.unwrap_or_default());
        let (from, to) = (self.name(h.from)?, self.name(h.to)?);
        Ok(format!("move {from} ahead of {to} in {g}, or let {g} run out of order: {} goal set {g} --in-order off", board::tb_cmd(app)))
    }
}

/// Why `t` may not wait for task `n`: `n` can't start until `t` is done, because of a goal's run-in-order
/// order somewhere on the way. None when it may (a cycle of waits alone is `waitsfor::clean`'s to refuse).
pub fn refusal(app: &App, t: &Row, n: i64) -> Result<Option<String>> {
    let mut w = Walk::new(app, Mode::Plan);
    w.tasks.insert(t.id(), Some(t.clone()));
    let Some(hops) = w.path(n, t.id())?.filter(|h| h.iter().any(|h| h.in_goal.is_some())) else { return Ok(None) };
    let (me, it) = (w.name(t.id())?, w.name(n)?);
    let (said, fix) = (w.say(&hops)?, w.fix(app, &hops)?);
    Ok(Some(format!(
        "{it} can't start until {me} is done ({said}), so {me} waiting for {it} would hold both forever. \
         For {me} to wait for {it}, {fix}."
    )))
}

/// A cycle on the board now: the queued task whose wait closes it, and its steps from there round.
struct Cycle {
    task: Row,
    hops: Vec<Hop>,
}

impl Cycle {
    fn key(&self) -> String {
        let mut ids: Vec<i64> = self.hops.iter().map(|h| h.from).collect();
        ids.sort();
        format!("{PREFIX}{}", ids.iter().map(|i| rf("task", *i)).collect::<Vec<_>>().join(","))
    }
}

/// The cycles holding queued tasks now, each once, found from the queued tasks that wait for another.
fn cycles(app: &App) -> Result<Vec<Cycle>> {
    let queued = app.db.q(
        "SELECT t.* FROM tasks t LEFT JOIN goals g ON g.id = t.goal_id WHERE t.status = 'queued' AND (t.waits_for IS NOT NULL OR t.pr_after IS NOT NULL) \
         AND COALESCE(g.paused, 0) = 0 AND COALESCE(g.deprioritized, 0) = 0 AND COALESCE(g.archived, 0) = 0 ORDER BY t.id",
        crate::p![],
    )?;
    let mut w = Walk::new(app, Mode::Live);
    let mut out: Vec<Cycle> = vec![];
    for x in queued {
        for first in w.holds(&x)?.into_iter().filter(|h| h.in_goal.is_none()) {
            let Some(rest) = w.path(first.to, x.id())? else { continue };
            if !rest.iter().any(|h| h.in_goal.is_some()) {
                continue;
            }
            let c = Cycle { task: x.clone(), hops: [vec![first], rest].concat() };
            if !out.iter().any(|o| o.key() == c.key()) {
                out.push(c);
            }
        }
    }
    Ok(out)
}

fn raise(app: &App, w: &mut Walk, c: &Cycle, key: &str) -> Result<()> {
    let me = w.name(c.task.id())?;
    let to = w.name(c.hops[0].to)?;
    let said = w.say(&c.hops)?;
    let detail = format!("These wait on each other, so none of them starts: {said}.");
    let text = format!(
        "{me}: {}. {detail} Drop the wait ({} wait-for --drop {to} --task {me}), or {}.",
        c.task.st("title"),
        board::tb_cmd(app),
        w.fix(app, &c.hops)?
    );
    let fields = json!({"title": c.task.st("title"), "detail": detail});
    board::log_event(app, c.task.id(), board::BOARD, "note", &format!("Stuck in a cycle with its goal's order: {said}"))?;
    crate::dispatch::add_alert_keyed_with(app, &text, Some(c.task.id()), c.task.i("goal_id"), key, fields)
}

/// Whether an alert is one of these: it stays up until `sweep` sees its cycle broken.
pub fn is_key(key: &str) -> bool {
    key.starts_with(PREFIX)
}

/// Puts each cycle of waits and goal order on Needs you, once per spell, and takes down the ones that
/// are broken. The number raised.
pub fn sweep(app: &App) -> Result<i64> {
    let before: Vec<String> = jloads_arr(crate::reqcache::setting(app, RAISED).ok().flatten().as_deref())
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    let up = crate::dispatch::alerts(app);
    let mut w = Walk::new(app, Mode::Live);
    let (mut now, mut sent) = (vec![], 0);
    for c in cycles(app)? {
        let key = c.key();
        if !before.contains(&key) && !up.iter().any(|a| a["key"] == key) {
            raise(app, &mut w, &c, &key)?;
            sent += 1;
        }
        now.push(key);
    }
    for a in &up {
        if let Some(k) = a["key"].as_str().filter(|k| is_key(k) && !now.iter().any(|n| n == k)) {
            crate::dispatch::clear_alert_key(app, k)?;
        }
    }
    if now != before {
        crate::reqcache::forget_setting(RAISED);
        app.db.set_setting(RAISED, if now.is_empty() { None } else { Some(jdumps(&Value::from(now))) }.as_deref())?;
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::fields;

    fn app(dir: &std::path::Path) -> std::sync::Arc<App> {
        App::for_tests(Config::for_tests(dir))
    }

    fn goal(app: &App, in_order: bool) -> i64 {
        app.db.insert("goals", fields!["name" => "Ship it", "run_in_order" => in_order as i64]).unwrap()
    }

    fn task(app: &App, g: i64, pos: i64, status: &str) -> i64 {
        app.db.insert("tasks", fields!["title" => format!("Step {pos}"), "goal_id" => g, "position" => pos, "status" => status]).unwrap()
    }

    fn waits(app: &App, t: i64, on: i64) {
        board::update_task(app, t, fields!["waits_for" => format!("[{on}]")]).unwrap();
    }

    fn row(app: &App, t: i64) -> Row {
        board::get_task(app, t).unwrap()
    }

    fn cycle_alerts(app: &App) -> Vec<Value> {
        crate::dispatch::alerts(app).into_iter().filter(|a| a["key"].as_str().is_some_and(is_key)).collect()
    }

    #[test]
    fn an_earlier_task_may_not_wait_for_a_later_one() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let g = goal(&app, true);
        let (a, b) = (task(&app, g, 1, "queued"), task(&app, g, 2, "queued"));
        let why = refusal(&app, &row(&app, a), b).unwrap().expect("refused");
        let (ra, rb, rg) = (rf("task", a), rf("task", b), rf("goal", g));
        assert!(why.contains(&format!("{rb} runs after {ra} in {rg}'s order")), "{why}");
        assert!(why.contains(&format!("goal set {rg} --in-order off")), "{why}");
        assert!(why.contains(&format!("move {rb} ahead of {ra}")), "{why}");
        assert!(refusal(&app, &row(&app, b), a).unwrap().is_none(), "a later task waits for an earlier one already");
    }

    #[test]
    fn the_refusal_goes_through_task_set() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let g = goal(&app, true);
        let (a, b) = (task(&app, g, 1, "planned"), task(&app, g, 2, "planned"));
        let e = waitsfor::clean(&app, &json!(rf("task", b)), Some(&row(&app, a))).unwrap_err();
        assert!(e.to_string().contains("would hold both forever"), "{e}");
        assert!(waitsfor::clean(&app, &json!(rf("task", a)), Some(&row(&app, b))).unwrap().is_some());
    }

    #[test]
    fn a_cycle_through_another_wait_is_refused_too() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let g = goal(&app, true);
        let (a, b) = (task(&app, g, 1, "queued"), task(&app, g, 2, "queued"));
        let other = app.db.insert("tasks", fields!["title" => "Elsewhere", "status" => "queued"]).unwrap();
        waits(&app, other, b);
        let why = refusal(&app, &row(&app, a), other).unwrap().expect("refused");
        assert!(why.contains(&format!("{} waits for {}", rf("task", other), rf("task", b))), "{why}");
    }

    #[test]
    fn order_doesnt_hold_out_of_order_finished_or_waved_goals() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let free = goal(&app, false);
        let (a, b) = (task(&app, free, 1, "queued"), task(&app, free, 2, "queued"));
        assert!(refusal(&app, &row(&app, a), b).unwrap().is_none(), "the goal doesn't run in order");

        let g = goal(&app, true);
        let (a, b) = (task(&app, g, 1, "queued"), task(&app, g, 2, "working"));
        assert!(refusal(&app, &row(&app, a), b).unwrap().is_none(), "the later task started already");

        let waved = goal(&app, true);
        let (a, b) = (task(&app, waved, 1, "queued"), task(&app, waved, 2, "queued"));
        board::update_task(&app, a, fields!["wave" => 1]).unwrap();
        board::update_task(&app, b, fields!["wave" => 2]).unwrap();
        assert!(refusal(&app, &row(&app, a), b).unwrap().is_none(), "waves take the place of the order");
    }

    #[test]
    fn an_existing_cycle_goes_on_needs_you_once_and_comes_down_when_broken() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let g = goal(&app, true);
        let (a, b) = (task(&app, g, 1, "queued"), task(&app, g, 2, "queued"));
        task(&app, g, 3, "queued");
        waits(&app, a, b);
        assert_eq!(sweep(&app).unwrap(), 1);
        let up = cycle_alerts(&app);
        assert_eq!(up.len(), 1, "{up:?}");
        assert_eq!(up[0]["task_id"].as_i64(), Some(a));
        assert_eq!(up[0]["goal"].as_str(), Some(rf("goal", g).as_str()));
        let (ra, rb) = (rf("task", a), rf("task", b));
        let text = up[0]["text"].as_str().unwrap();
        assert!(text.contains(&format!("{ra} waits for {rb}, {rb} runs after {ra} in {}'s order", rf("goal", g))), "{text}");
        assert!(text.contains(&format!("wait-for --drop {rb} --task {ra}")), "{text}");
        crate::dispatch::prune_alerts(&app).unwrap();
        assert_eq!(cycle_alerts(&app).len(), 1, "it stays until the cycle is broken");

        assert_eq!(sweep(&app).unwrap(), 0, "raised once");
        crate::dispatch::clear_alert_key(&app, up[0]["key"].as_str().unwrap()).unwrap();
        assert_eq!(sweep(&app).unwrap(), 0, "a dismissed alert isn't raised again while the cycle stands");

        board::update_task(&app, a, fields!["waits_for" => null]).unwrap();
        assert_eq!(sweep(&app).unwrap(), 0);
        waits(&app, a, b);
        assert_eq!(sweep(&app).unwrap(), 1, "a new spell is raised again");
        app.db.x("UPDATE goals SET run_in_order = 0 WHERE id = ?", crate::p![g]).unwrap();
        sweep(&app).unwrap();
        assert!(cycle_alerts(&app).is_empty(), "comes down once the goal runs out of order");
    }

    #[test]
    fn a_wait_the_order_already_keeps_isnt_flagged() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let g = goal(&app, true);
        let (a, b) = (task(&app, g, 1, "queued"), task(&app, g, 2, "queued"));
        waits(&app, b, a);
        let paused = app.db.insert("goals", fields!["name" => "Resting", "run_in_order" => 1, "paused" => 1]).unwrap();
        let (c, e) = (task(&app, paused, 1, "queued"), task(&app, paused, 2, "queued"));
        waits(&app, c, e);
        assert_eq!(sweep(&app).unwrap(), 0);
        assert!(cycle_alerts(&app).is_empty());
    }
}
