//! Waits that cycle with the order goals run their tasks in. A task can't start until the tasks before it
//! are done when its goal runs in order, until every task of a goal its goal runs after (`--after`) is done,
//! and until an earlier wave that stops for the owner's review is done. So an earlier task that waits for
//! a later one (or for anything that comes to wait on it) holds both forever, and the rest of the goal
//! behind them. A wait that would make such a cycle is refused (`refusal`), and one already on the board
//! (the order changed, or the goal was set to run in order after the wait) goes on Needs you until it's
//! broken (`sweep`). Waves alone don't: a wave passes once its tasks are held back by their own waits.

use std::collections::{HashMap, VecDeque};

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;
use crate::waitsfor;

const PREFIX: &str = "ordercycle:";
const RAISED: &str = "order_cycles_raised";

/// One step of a cycle: `from` can't start until `to` is done, for the reason `by` gives.
#[derive(Clone, Debug)]
struct Hop {
    from: i64,
    to: i64,
    by: By,
}

#[derive(Clone, Debug, PartialEq)]
enum By {
    /// `from` waits for `to`, or stacks on it.
    Wait,
    /// `to` comes before `from` in this goal's run-in-order order.
    Order(i64),
    /// `from`'s goal (the first) runs after `to`'s (the second).
    Chain(i64, i64),
    /// `to` is in this goal's wave (the second) that stops for the owner's review before `from`'s wave.
    Stop(i64, i64),
}

impl Hop {
    /// Held by the goals' order, not by a wait.
    fn by_order(&self) -> bool {
        self.by != By::Wait
    }
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
    after: HashMap<i64, Vec<(i64, Vec<Row>)>>,
    stops: HashMap<i64, Vec<Row>>,
}

impl<'a> Walk<'a> {
    fn new(app: &'a App, mode: Mode) -> Self {
        Walk { app, mode, tasks: HashMap::new(), order: HashMap::new(), after: HashMap::new(), stops: HashMap::new() }
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

    /// The goals this one runs after that aren't finished, each with its tasks (`chain::blocker`).
    fn after(&mut self, goal: i64) -> Result<&[(i64, Vec<Row>)]> {
        if !self.after.contains_key(&goal) {
            let mut rows = vec![];
            if let Some(g) = board::find_goal(self.app, Some(goal))? {
                for n in crate::chain::ids(&g) {
                    if board::find_goal(self.app, Some(n))?.is_some() && !board::goal_finished(self.app, n)? {
                        rows.push((n, crate::shared::counted_tasks(self.app, n)?));
                    }
                }
            }
            self.after.insert(goal, rows);
        }
        Ok(&self.after[&goal])
    }

    /// The goal's tasks in a wave that stops for the owner's review and isn't continued yet: a later
    /// wave waits until every one of them is done (`waves::waves`).
    fn stopped(&mut self, goal: i64) -> Result<&[Row]> {
        if !self.stops.contains_key(&goal) {
            let stops: Vec<i64> = self
                .app
                .db
                .q("SELECT wave FROM goal_waves WHERE goal_id = ? AND stop_after = 1 AND released_at IS NULL", crate::p![goal])?
                .iter()
                .filter_map(|w| w.i("wave"))
                .collect();
            let rows = if stops.is_empty() {
                vec![]
            } else {
                board::goal_tasks(self.app, goal)?.into_iter().filter(|t| t.i("wave").is_some_and(|w| stops.contains(&w))).collect()
            };
            self.stops.insert(goal, rows);
        }
        Ok(&self.stops[&goal])
    }

    /// Whether `y` isn't done yet, so it holds back the tasks its goals' order puts after it.
    fn pending(&self, y: &Row) -> bool {
        let open = match y.s("status") {
            Some("done") => false,
            Some("planned") => self.mode == Mode::Plan,
            _ => true,
        };
        open && !crate::human::released(y)
    }

    /// Whether `x` can be held back at all: a queued task, or in `Plan` a planned one too.
    fn waits(&self, x: &Row) -> bool {
        match x.s("status") {
            Some("queued") => true,
            Some("planned") => self.mode == Mode::Plan,
            _ => false,
        }
    }

    /// What holds `x` back: the tasks it waits for that aren't ready, the earlier tasks in its goal's
    /// order, the tasks of the goals its goal runs after, and those of an earlier wave that stops for
    /// review, that aren't done.
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
                out.push(Hop { from: x.id(), to: d, by: By::Wait });
            }
        }
        // A goal that runs after others keeps its tasks planned until those finish (`chain::finished`).
        let Some(g) = x.i("goal_id").filter(|_| self.waits(x) || x.s("status") == Some("planned")) else { return Ok(out) };
        let mut ahead: Vec<(Row, By)> = vec![];
        for (n, rows) in self.after(g)? {
            ahead.extend(rows.iter().map(|y| (y.clone(), By::Chain(g, *n))));
        }
        if !self.waits(x) {
            return Ok(self.hops(x, out, ahead));
        }
        for y in self.in_order(g)? {
            if y.id() == x.id() {
                break;
            }
            ahead.push((y.clone(), By::Order(g)));
        }
        if let Some(mine) = x.i("wave") {
            for y in self.stopped(g)? {
                if let Some(w) = y.i("wave").filter(|w| *w < mine) {
                    ahead.push((y.clone(), By::Stop(g, w)));
                }
            }
        }
        Ok(self.hops(x, out, ahead))
    }

    /// `out` and a step to each task in `ahead` that isn't done yet.
    fn hops(&self, x: &Row, mut out: Vec<Hop>, ahead: Vec<(Row, By)>) -> Vec<Hop> {
        for (y, by) in ahead {
            if y.id() != x.id() && self.pending(&y) {
                out.push(Hop { from: x.id(), to: y.id(), by });
            }
        }
        out
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
            bits.push(match h.by {
                By::Wait => format!("{from} waits for {to}"),
                By::Order(g) => format!("{from} runs after {to} in {}'s order", rf("goal", g)),
                By::Chain(g, n) => format!("{from} is in {}, which runs after {to}'s goal {}", rf("goal", g), rf("goal", n)),
                By::Stop(g, w) => {
                    format!("{from} is in a later wave than {to}, and {} stops after wave {w} until it's done and {} reviews it", rf("goal", g), self.app.cfg.owner)
                }
            });
        }
        Ok(bits.join(", "))
    }

    /// How to break the cycle at the first step the goals' order holds: move the pair, or lift the order.
    fn fix(&mut self, app: &App, hops: &[Hop]) -> Result<String> {
        let Some(h) = hops.iter().find(|h| h.by_order()) else { return Ok(String::new()) };
        let (from, to, tb) = (self.name(h.from)?, self.name(h.to)?, board::tb_cmd(app));
        Ok(match h.by {
            By::Wait => String::new(),
            By::Order(g) => {
                let g = rf("goal", g);
                format!("move {from} ahead of {to} in {g}, or let {g} run out of order: {tb} goal set {g} --in-order off")
            }
            By::Chain(g, n) => {
                let rest: Vec<String> = board::find_goal(app, Some(g))?
                    .map(|x| crate::chain::ids(&x))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|o| *o != n)
                    .map(|o| rf("goal", o))
                    .collect();
                let after = if rest.is_empty() { "none".to_string() } else { rest.join(" --after ") };
                let (g, n) = (rf("goal", g), rf("goal", n));
                format!("move {to} out of {n} or {from} out of {g}, or let {g} run without waiting for {n}: {tb} goal set {g} --after {after}")
            }
            By::Stop(g, w) => format!(
                "move {from} into wave {w} or earlier: {tb} task set {from} --wave {w}, or {} takes the review stop off {}'s wave {w}",
                app.cfg.owner,
                rf("goal", g)
            ),
        })
    }
}

/// Why `t` may not wait for task `n`: `n` can't start until `t` is done, because of the goals' order
/// somewhere on the way (run in order, `--after`, a review stop). None when it may (a cycle of waits
/// alone is `waitsfor::clean`'s to refuse).
pub fn refusal(app: &App, t: &Row, n: i64) -> Result<Option<String>> {
    let mut w = Walk::new(app, Mode::Plan);
    w.tasks.insert(t.id(), Some(t.clone()));
    let Some(hops) = w.path(n, t.id())?.filter(|h| h.iter().any(|h| h.by_order())) else { return Ok(None) };
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
        for first in w.holds(&x)?.into_iter().filter(|h| !h.by_order()) {
            let Some(rest) = w.path(first.to, x.id())? else { continue };
            if !rest.iter().any(|h| h.by_order()) {
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

    fn after(app: &App, g: i64, first: i64) {
        app.db.x("UPDATE goals SET after_goals = ? WHERE id = ?", crate::p![format!("[{first}]"), g]).unwrap();
    }

    #[test]
    fn a_task_may_not_wait_for_one_in_a_goal_that_runs_after_its_own() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let (first, then) = (goal(&app, false), goal(&app, false));
        after(&app, then, first);
        let (a, b) = (task(&app, first, 1, "queued"), task(&app, then, 1, "planned"));
        let why = refusal(&app, &row(&app, a), b).unwrap().expect("refused");
        let (ra, rb, rf1, rf2) = (rf("task", a), rf("task", b), rf("goal", first), rf("goal", then));
        assert!(why.contains(&format!("{rb} is in {rf2}, which runs after {ra}'s goal {rf1}")), "{why}");
        assert!(why.contains(&format!("goal set {rf2} --after none")), "{why}");
        let e = waitsfor::clean(&app, &json!(rb), Some(&row(&app, a))).unwrap_err();
        assert!(e.to_string().contains("would hold both forever"), "{e}");
        assert!(crate::stack::clean(&app, &json!(rb), Some(&row(&app, a))).is_err(), "stacking on it holds both too");
        assert!(refusal(&app, &row(&app, b), a).unwrap().is_none(), "a later goal's task waits for an earlier goal's already");

        board::update_task(&app, a, fields!["status" => "done"]).unwrap();
        let c = task(&app, first, 2, "queued");
        assert!(refusal(&app, &row(&app, c), b).unwrap().is_some(), "the earlier goal isn't finished while one of its tasks is open");
        board::update_task(&app, c, fields!["status" => "done"]).unwrap();
        let other = goal(&app, false);
        let x = task(&app, other, 1, "queued");
        assert!(refusal(&app, &row(&app, x), b).unwrap().is_none(), "the goal it runs after is finished");
    }

    #[test]
    fn a_wait_past_a_review_stop_is_refused_but_waves_alone_dont_hold() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let g = goal(&app, false);
        let (a, b) = (task(&app, g, 1, "queued"), task(&app, g, 2, "queued"));
        board::update_task(&app, a, fields!["wave" => 1]).unwrap();
        board::update_task(&app, b, fields!["wave" => 2]).unwrap();
        assert!(refusal(&app, &row(&app, a), b).unwrap().is_none(), "wave 1 passes once its task waits for its own");

        crate::waves::set_wave(&app, g, 1, fields!["stop_after" => 1]).unwrap();
        let why = refusal(&app, &row(&app, a), b).unwrap().expect("refused");
        let (ra, rb, rg) = (rf("task", a), rf("task", b), rf("goal", g));
        assert!(why.contains(&format!("{rb} is in a later wave than {ra}, and {rg} stops after wave 1")), "{why}");
        assert!(why.contains(&format!("task set {rb} --wave 1")), "{why}");
        assert!(refusal(&app, &row(&app, b), a).unwrap().is_none(), "the later wave waits for the earlier already");

        crate::waves::set_wave(&app, g, 1, fields!["released_at" => now_iso()]).unwrap();
        assert!(refusal(&app, &row(&app, a), b).unwrap().is_none(), "continued past the stop");
    }

    #[test]
    fn an_existing_wait_into_a_later_goal_goes_on_needs_you() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let (first, then) = (goal(&app, false), goal(&app, false));
        let (a, b) = (task(&app, first, 1, "queued"), task(&app, then, 1, "planned"));
        waits(&app, a, b);
        assert_eq!(sweep(&app).unwrap(), 0, "no cycle while the goals run side by side");
        after(&app, then, first);
        assert_eq!(sweep(&app).unwrap(), 1);
        let up = cycle_alerts(&app);
        assert_eq!(up.len(), 1, "{up:?}");
        assert_eq!(up[0]["task_id"].as_i64(), Some(a));
        let text = up[0]["text"].as_str().unwrap();
        let (ra, rb) = (rf("task", a), rf("task", b));
        assert!(text.contains(&format!("{ra} waits for {rb}, {rb} is in {}", rf("goal", then))), "{text}");
        app.db.x("UPDATE goals SET after_goals = NULL WHERE id = ?", crate::p![then]).unwrap();
        sweep(&app).unwrap();
        assert!(cycle_alerts(&app).is_empty(), "comes down once the goal no longer runs after the other");
    }
}
