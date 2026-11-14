//! One stop state for a goal the owner has to move: the sidebar, the Goals page, the goal page and
//! its waves read it the same way. A stop is a failed task holding a wave, a held wave, a task
//! waiting on an answer, a wave done and stopped for review, or an outcome missed after every task
//! is done. The first one in that order is the goal's stop; each says what it is, in a word and a
//! glyph so it never rests on colour, and offers its next step.
use crate::fmt::{self, arr, i, opt_s, s};
use crate::theme::Theme;
use crate::ui::status::{self, St};
use gpui_kit::Hsla;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Failed,
    Held,
    Answer,
    Review,
    Missed,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Failed => "Failed",
            Kind::Held => "Held",
            Kind::Answer => "Needs your answer",
            Kind::Review => "Ready for review",
            Kind::Missed => "Outcome missed",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Kind::Failed => "×",
            Kind::Held => "‖",
            Kind::Answer => "?",
            Kind::Review => "◐",
            Kind::Missed => "!",
        }
    }

    /// "‖ Held".
    pub fn label(self) -> String {
        format!("{} {}", self.glyph(), self.word())
    }

    /// Red for a failure, orange for the rest: each waits on the owner.
    pub fn bad(self) -> bool {
        self == Kind::Failed
    }

    /// (text, soft fill, line) in the theme.
    pub fn colors(self, t: &Theme) -> (Hsla, Hsla, Hsla) {
        if self.bad() { (t.down, t.down_soft, t.down_line) } else { (t.warn_fg, t.warn_soft, t.warn_line) }
    }
}

/// What a stop's button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// `POST goals/:g/waves/:n/continue`: let a held wave start, or go on past a done or failed one.
    Continue(i64),
    /// Open the task in the right panel.
    Task(String),
    /// List the misses holding the goal open.
    Misses,
    /// Open the goal's page, where the tasks waiting on the owner are.
    Goal,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub kind: Kind,
    /// The wave it stopped at.
    pub wave: Option<i64>,
    /// A row's line: "Wave 2 held", "T54 failed · wave 3 waits".
    pub line: String,
    /// The goal page's band: "Wave 2 is held." and what to do about it.
    pub title: String,
    pub detail: String,
    /// The next step, then the other way on.
    pub primary: (String, Step),
    pub secondary: Option<(String, Step)>,
    /// A wave stopped for review offers its review beside Continue.
    pub review: bool,
}

/// "wave 3", or "the rest of the goal" when the stop is at the last wave.
fn onward(g: &Value, n: i64) -> String {
    let ws = arr(g, "waves");
    if ws.is_empty() || ws.iter().any(|w| i(w, "wave") == n + 1) { format!("wave {}", n + 1) } else { "the rest of the goal".into() }
}

/// How many tasks wait on the owner's answer, and the first: from the goal's tasks (a lost or
/// closed terminal reads stopped, not waiting on you), else its summary's count and peek.
fn asking(g: &Value) -> (i64, Option<String>) {
    let first = |ts: Vec<&Value>| ts.first().and_then(|t| opt_s(t, "ref")).map(str::to_string);
    match g.get("tasks").and_then(Value::as_array) {
        Some(ts) => {
            let asks: Vec<&Value> = ts.iter().filter(|t| status::row(t).st == St::Needs).collect();
            (asks.len() as i64, first(asks))
        }
        None => (i(g, "needs"), first(arr(g, "peek").iter().filter(|t| s(t, "status") == "needs").collect())),
    }
}

/// Every task done (open PRs aside) yet misses on the done-when keep the goal open.
fn missed(g: &Value) -> Option<i64> {
    let misses = i(g, "misses");
    let all_done = match g.get("tasks").and_then(Value::as_array) {
        Some(ts) => !ts.is_empty() && ts.iter().all(|t| s(t, "status") == "done"),
        None => i(g, "total") > 0 && i(g, "done") >= i(g, "total"),
    };
    (misses > 0 && all_done).then_some(misses)
}

/// The goal's stop, if it has one. A paused goal shows as paused, not stopped.
pub fn of(g: &Value) -> Option<Stop> {
    if fmt::b(g, "paused") {
        return None;
    }
    let sig = g.get("stop").filter(|v| v.is_object());
    let wave = sig.and_then(|v| v["wave"].as_i64());
    match (sig.map(|v| s(v, "kind")), wave) {
        (Some("failed"), Some(n)) => {
            let failed: Vec<&str> = sig.map(|v| arr(v, "tasks")).unwrap_or(&[]).iter().filter_map(Value::as_str).collect();
            let who = if failed.is_empty() { "A task".to_string() } else { failed.join(", ") };
            let on = onward(g, n);
            let primary = match failed.first() {
                Some(r) => (format!("Open {r}"), Step::Task(r.to_string())),
                None => ("Continue anyway".into(), Step::Continue(n)),
            };
            let secondary = (!failed.is_empty()).then(|| ("Continue anyway".to_string(), Step::Continue(n)));
            return Some(Stop {
                kind: Kind::Failed,
                wave,
                line: format!("{who} failed · {on} waits"),
                title: format!("{who} failed in wave {n}."),
                detail: format!("{} waits. Try it again from its panel, or continue past it.", fmt::cap(&on)),
                primary,
                secondary,
                review: false,
            });
        }
        (Some("held"), Some(n)) => {
            let at = sig.and_then(|v| opt_s(v, "at")).map(fmt::day_time).filter(|x| !x.is_empty());
            let detail = match at {
                Some(at) => format!("It starts only when someone lets it go. Held at {at}."),
                None => "It starts only when someone lets it go.".into(),
            };
            return Some(Stop {
                kind: Kind::Held,
                wave,
                line: format!("Wave {n} held"),
                title: format!("Wave {n} is held."),
                detail,
                primary: (format!("Let wave {n} start"), Step::Continue(n)),
                secondary: None,
                review: false,
            });
        }
        _ => {}
    }
    if let (n @ 1.., who) = asking(g) {
        let (line, title) = match (&who, n) {
            (Some(r), 1) => (format!("{r} waits on your answer"), format!("{r} needs your answer.")),
            _ => (format!("{} need you", fmt::plural(n, "task", "tasks")), format!("{} need your answer.", fmt::plural(n, "task", "tasks"))),
        };
        let primary = match who {
            Some(r) => (format!("Answer {r}"), Step::Task(r)),
            None => ("Answer".into(), Step::Goal),
        };
        return Some(Stop { kind: Kind::Answer, wave: None, line, title, detail: "Its work waits until you answer.".into(), primary, secondary: None, review: false });
    }
    if let (Some("review"), Some(n)) = (sig.map(|v| s(v, "kind")), wave) {
        let on = onward(g, n);
        return Some(Stop {
            kind: Kind::Review,
            wave,
            line: format!("Wave {n} done · look it over"),
            title: format!("Wave {n} is done."),
            detail: format!("Look it over, then let {on} start."),
            primary: (format!("Continue to {on}"), Step::Continue(n)),
            secondary: None,
            review: true,
        });
    }
    let n = missed(g)?;
    let misses = fmt::plural(n, "miss", "misses");
    Some(Stop {
        kind: Kind::Missed,
        wave: None,
        line: format!("{misses} hold the goal open"),
        title: format!("Every task is done, but {misses} {} this goal open.", if n == 1 { "holds" } else { "hold" }),
        detail: "Fix or settle each one to finish the goal.".into(),
        primary: ("See the misses".into(), Step::Misses),
        secondary: None,
        review: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_held_wave_reads_held_and_lets_it_start() {
        let g = json!({"stop": {"kind": "held", "wave": 2, "tasks": [], "at": null}, "waves": [{"wave": 1}, {"wave": 2}, {"wave": 3}]});
        let st = of(&g).unwrap();
        assert_eq!((st.kind, st.line.as_str(), st.title.as_str()), (Kind::Held, "Wave 2 held", "Wave 2 is held."));
        assert_eq!(st.primary, ("Let wave 2 start".to_string(), Step::Continue(2)));
        assert_eq!(st.kind.label(), "‖ Held");
    }

    #[test]
    fn a_failed_wave_opens_its_task_or_goes_on_anyway() {
        let g = json!({"stop": {"kind": "failed", "wave": 2, "tasks": ["T54"]}, "waves": [{"wave": 2}, {"wave": 3}]});
        let st = of(&g).unwrap();
        assert_eq!(st.line, "T54 failed · wave 3 waits");
        assert_eq!(st.primary, ("Open T54".to_string(), Step::Task("T54".into())));
        assert_eq!(st.secondary, Some(("Continue anyway".to_string(), Step::Continue(2))));
        assert!(st.kind.bad());
    }

    #[test]
    fn a_review_stop_continues_to_the_next_wave_with_its_review() {
        let g = json!({"stop": {"kind": "review", "wave": 1}, "waves": [{"wave": 1}, {"wave": 2}]});
        let st = of(&g).unwrap();
        assert_eq!((st.kind, st.line.as_str()), (Kind::Review, "Wave 1 done · look it over"));
        assert_eq!(st.primary.0, "Continue to wave 2");
        assert!(st.review);
        let last = json!({"stop": {"kind": "review", "wave": 2}, "waves": [{"wave": 1}, {"wave": 2}]});
        assert_eq!(of(&last).unwrap().primary.0, "Continue to the rest of the goal");
    }

    #[test]
    fn stops_come_in_one_order_and_a_paused_goal_has_none() {
        let both = json!({"stop": {"kind": "review", "wave": 1}, "needs": 1, "peek": [{"ref": "T3", "status": "needs"}]});
        let st = of(&both).unwrap();
        assert_eq!((st.kind, st.primary.0.as_str()), (Kind::Answer, "Answer T3"));
        assert!(Kind::Failed < Kind::Held && Kind::Held < Kind::Answer && Kind::Answer < Kind::Review && Kind::Review < Kind::Missed);
        assert_eq!(of(&json!({"paused": true, "stop": {"kind": "held", "wave": 1}})), None);
        assert_eq!(of(&json!({"done": 2, "total": 4})), None);
    }

    #[test]
    fn a_missed_outcome_is_not_finished() {
        let g = json!({"done": 3, "total": 3, "misses": 2});
        let st = of(&g).unwrap();
        assert_eq!((st.kind, st.line.as_str()), (Kind::Missed, "2 misses hold the goal open"));
        assert_eq!(st.title, "Every task is done, but 2 misses hold this goal open.");
        assert_eq!(st.primary, ("See the misses".to_string(), Step::Misses));
        assert_eq!(of(&json!({"done": 2, "total": 3, "misses": 1})), None, "work left: not a miss yet");
    }
}
