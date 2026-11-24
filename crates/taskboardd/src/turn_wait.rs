//! A turn that ends saying the task waits on other board tasks ("T12 is now waiting for T15 and T16;
//! once both are in, I'll…") without running `tb wait-for`. Nothing records that wait, so nothing brings
//! the conversation back when they finish, and the idle terminal would sit on the owner as "Waiting for
//! you". The Stop hook sends the agent back once with the exact `tb wait-for` line; when its turn ends
//! again still saying it waits, the board records the wait itself, as `tb wait-for` would.
//!
//! The same goes for a turn that ends saying the task waits until a set time ("Trigger t_b3b364 wakes
//! this terminal at 06:30 UTC. Then I'll check pipeline 16…") without `tb wait-until` ([`until`]):
//! sent back once with the exact line, parked by the board when it ends that way again.

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::app::App;
use crate::board;
use crate::util::*;
use crate::waitsfor;

/// What a turn's reply says the task waits on.
#[derive(Debug, Default, PartialEq)]
pub struct Waits {
    /// The tasks it may wait for, as refs (T15, H4).
    pub refs: Vec<String>,
    /// Their ids, in the same order.
    pub ids: Vec<i64>,
    /// It says it needs them merged, not just done.
    pub merged: bool,
    /// Tasks it can't wait for, each with the board's reason.
    pub refused: Vec<String>,
    /// The paragraph that says so.
    pub said: String,
}

static WAITING: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b(wait(s|ing)?\s+(for|on|until)|blocked\s+(on|by)|parked\s+on|depends\s+on|until\b[^.]{0,80}\b(is|are)\s+(done|in|merged|finished|landed|ready|shipped|complete)|once\b[^.]{0,120}\b(is|are|has|have|gets?)\s+(done|in|merged|finished|landed|ready|shipped|complete|completed)|needs?\b[^.]{0,60}\b(first|to\s+(finish|land|merge)))\b",
    )
    .unwrap()
});

static MERGED: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bmerge[sd]?\b").unwrap());

static REF: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b([TH])(\d+)\b").unwrap());

/// The board tasks `reply` (a turn's reply end) says `t` waits on and that aren't ready for it yet: done
/// (merged, when it says it needs them merged) or already waited for don't count. Only paragraphs that
/// read as waiting are read. None when it names none.
pub fn waits_on(app: &App, t: &Row, reply: &str) -> Result<Option<Waits>> {
    let have = waitsfor::ids(t);
    let mut w = Waits::default();
    for para in reply.split("\n\n").map(str::trim).filter(|p| WAITING.is_match(p)) {
        let merged = MERGED.is_match(para);
        let mut named = false;
        for c in REF.captures_iter(para) {
            let Ok(n) = c[2].parse::<i64>() else { continue };
            if n == t.id() || w.ids.contains(&n) || have.contains(&n) {
                continue;
            }
            let Some(o) = board::find_task(app, Some(n))? else { continue };
            let r = crate::human::task_ref(&o);
            if r != c[0] || w.refused.iter().any(|x| x.starts_with(&format!("{r}:"))) {
                continue;
            }
            let open = if merged { !waitsfor::merged(app, Some(&o))? } else { !waitsfor::ready(Some(&o)) };
            if !open || (o.s("status") == Some("done") && o.b("failed")) {
                continue;
            }
            named = true;
            match waitsfor::clean(app, &serde_json::json!([n]), Some(t)) {
                Ok(_) => {
                    w.refs.push(r);
                    w.ids.push(n);
                    w.merged |= merged;
                }
                Err(e) => w.refused.push(format!("{r}: {}", e.message)),
            }
        }
        if named && w.said.is_empty() {
            w.said = one_line(para, 300);
        }
    }
    Ok(if w.ids.is_empty() && w.refused.is_empty() { None } else { Some(w) })
}

/// The `tb wait-for` line that records the wait.
pub fn line(app: &App, w: &Waits, mine: &str) -> String {
    format!(
        "{} wait-for {}{} --why \"<what {mine} needs from {}>\"",
        board::tb_cmd(app),
        w.refs.join(" "),
        if w.merged { " --merged" } else { "" },
        if w.refs.len() == 1 { "it" } else { "them" }
    )
}

/// What the Stop hook tells an agent it sends back.
pub fn reason(app: &App, w: &Waits, mine: &str) -> String {
    let owner = &app.cfg.owner;
    let mut out = vec![];
    if !w.ids.is_empty() {
        let them = crate::dispatch::say_refs(&w.refs);
        let done = match (w.refs.len(), w.merged) {
            (1, true) => "its PR merges",
            (1, false) => "it's done",
            (_, true) => "their PRs merge",
            (_, false) => "they're done",
        };
        out.push(format!(
            "Task board: your reply says {mine} is waiting on {them}, but nothing on the board records that wait, so nothing \
             brings this conversation back once {done}, and {mine} would sit on {owner} instead. Record it now and end your turn:\n  {}\n\
             If {mine} doesn't need {} after all, carry on with the work. If you end your turn still saying you wait for {}, \
             the board records the wait for you.",
            line(app, w, mine),
            if w.refs.len() == 1 { "it" } else { "them" },
            if w.refs.len() == 1 { "it" } else { "them" },
        ));
    }
    for r in &w.refused {
        out.push(format!("Task board: your reply says {mine} is waiting on {r}"));
    }
    out.join("\n\n")
}

/// What a turn's reply says about a set time its task waits until.
#[derive(Debug, PartialEq)]
pub struct Until {
    /// The time as `tb wait-until` reads it: "06:30 UTC", "4:15 am", "2026-11-24T04:15".
    pub time: String,
    /// When that is (seconds).
    pub at: f64,
    /// What it says it does then.
    pub plan: String,
}

/// Words that say the task comes back at a time: something wakes it, it picks the work back up, it
/// checks back. The time that follows is when it resumes.
static RESUMES: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b(wakes?|waking|wake\s+up|resumes?|resuming|picks?\s+(it|this|that|things|the\s+\w+)\s+(back\s+)?up|pick\s+(it|this|that|things)\s+(back\s+)?up|(check|look|come)(s|ing)?\s+(back|again)|be\s+back|back\s+(at|by)|(i'll|i\s+will|it'll|it\s+will)\s+(check|look|carry\s+on|continue|resume))\b",
    )
    .unwrap()
});

/// Words that say the task waits until a time without saying what wakes it: "waiting until 06:30",
/// "nothing more can be checked before the 05:00 UTC run".
static PARKS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b((wait(s|ing)?|parked|idle|sleep(s|ing)?|paused?)\s+(here\s+)?(until|till)|nothing\s+(more|else)\b[^.]{0,60}?\b(until|till|before))\b").unwrap()
});

/// A clock time, maybe in UTC, maybe with a day: "06:30 UTC", "4:15 AM Nov 24", "tomorrow 9am".
static CLOCK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b(?P<tom>tomorrow\s+(at\s+)?)?(?P<t>\d{1,2}:[0-5]\d(\s*[ap]\.?\s?m\b\.?)?|\d{1,2}\s*[ap]\.?\s?m\b\.?)(?P<utc>\s*(utc|gmt|z)\b)?(,?\s+(on\s+)?([a-z]+day,?\s+)?(?P<mon>jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+(?P<day>\d{1,2})\b|(?P<tom2>\s+tomorrow)\b)?",
    )
    .unwrap()
});

static SENTENCE_END: Lazy<Regex> = Lazy::new(|| Regex::new(r"[.!?](\s|$)").unwrap());

static THEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(then,?\s+)?((i'll|i\s+will|it'll|it\s+will|this\s+terminal\s+will)\s+)?").unwrap());

/// How far after the words that say it resumes the time may be.
const NEAR: usize = 80;

/// The set time `reply` (a turn's reply end) says its task waits until, read at `now`: the time after
/// words that say something wakes it or it picks the work back up, else after words that say it waits
/// until then. The last paragraph that says so wins. None when it names no time still to come.
pub fn until(reply: &str, now: f64) -> Option<Until> {
    reply.rsplit("\n\n").map(str::trim).find_map(|para| {
        [&*RESUMES, &*PARKS].iter().find_map(|re| {
            re.find_iter(para).find_map(|m| {
                let rest = &para[m.end()..];
                let c = CLOCK.captures(rest).filter(|c| c.get(0).is_some_and(|x| x.start() <= NEAR))?;
                let time = clock_text(&c, now)?;
                let at = crate::wait_until::parse(&time, now).ok()?;
                let end = m.end() + c.get(0)?.end();
                Some(Until { time, at, plan: plan_after(para, end) })
            })
        })
    })
}

/// The time a [`CLOCK`] match names, as `tb wait-until` reads it.
fn clock_text(c: &regex::Captures, now: f64) -> Option<String> {
    let t = crate::scheduled::spoken(&c["t"]);
    let utc = c.name("utc").is_some();
    let (Some(mon), Some(day)) = (c.name("mon"), c.name("day")) else {
        let tomorrow = c.name("tom").is_some() || c.name("tom2").is_some();
        return Some(format!("{}{t}{}", if tomorrow { "tomorrow " } else { "" }, if utc { " UTC" } else { "" }));
    };
    let mon = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| mon.as_str().to_lowercase().starts_with(m))? as u32 + 1;
    let day: u32 = day.as_str().parse().ok()?;
    let now_dt = Utc.timestamp_opt(now.floor() as i64, 0).single()?;
    let today = if utc { now_dt.date_naive() } else { now_dt.with_timezone(&Local).date_naive() };
    let mut date = NaiveDate::from_ymd_opt(today.year(), mon, day)?;
    if date < today {
        date = NaiveDate::from_ymd_opt(today.year() + 1, mon, day)?;
    }
    let at = crate::scheduled::parse_at(&t, &(date.and_hms_opt(0, 0, 0)? - Duration::seconds(1))).ok()?;
    Some(if utc { format!("{}Z", at.format("%Y-%m-%dT%H:%M:00")) } else { at.format("%Y-%m-%dT%H:%M").to_string() })
}

/// What a paragraph says it does once the time comes: the sentences after the one naming the time
/// ("Then I'll check pipeline 16…"), else the whole paragraph.
fn plan_after(para: &str, from: usize) -> String {
    let rest = SENTENCE_END.find(&para[from..]).map(|m| para[from + m.end()..].trim()).unwrap_or("");
    let plan = if rest.chars().any(char::is_alphanumeric) { rest } else { para };
    let plan = one_line(plan, 300);
    let skip = THEN.find(&plan).map(|m| m.end()).unwrap_or(0);
    plan[skip..].to_string()
}

/// The `tb wait-until` line that parks it, with nothing in it a shell would run.
pub fn until_line(app: &App, u: &Until) -> String {
    format!("{} wait-until \"{}\" --then \"{}\"", board::tb_cmd(app), u.time, quotable(&u.plan))
}

/// `text` safe inside a shell's double quotes: no backticks, `$` or backslashes, and `'` for `"`.
fn quotable(text: &str) -> String {
    let s: String = text.chars().filter(|c| !matches!(c, '`' | '$' | '\\')).map(|c| if c == '"' { '\'' } else { c }).collect();
    s.trim().to_string()
}

/// What the Stop hook tells an agent whose reply says it waits until a time it never parked on.
pub fn until_reason(app: &App, u: &Until, mine: &str) -> String {
    let owner = &app.cfg.owner;
    format!(
        "Task board: your reply says {mine} waits until {}, but nothing on the board records that, so {mine} would sit on {owner} \
         under Needs you as if it asked something. Park it now and end your turn:\n  {}\n\
         Fix the time or the plan if they're read wrong. If {mine} doesn't wait after all, carry on with the work. If you end your \
         turn still saying it waits until then, the board parks it for you. Whatever you set to wake it (a trigger, a scheduled \
         wake-up) still has to.",
        crate::wait_until::say(&iso(u.at)),
        until_line(app, u),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_reads_as_waiting_and_a_plain_mention_does_not() {
        for p in [
            "T2112 is now waiting for two results: T2115 and T2132. Once both are in, I'll rebase.",
            "Blocked on T15 until it lands.",
            "Once T15 is merged I'll rebase onto it.",
            "This needs T15 to finish first.",
            "I'll pick this up once T15's PR has landed.",
        ] {
            assert!(WAITING.is_match(p), "{p}");
        }
        for p in ["I fixed the bug T15 reported.", "T15 is done, so I rebased onto it.", "I waited for the build."] {
            assert!(!WAITING.is_match(p), "{p}");
        }
    }

    #[test]
    fn refs_are_whole_words() {
        let got: Vec<String> = REF.captures_iter("T15, H4 and AT9 or T12x").map(|c| c[0].to_string()).collect();
        assert_eq!(got, ["T15", "H4"]);
    }

    fn ts(s: &str) -> f64 {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp() as f64
    }

    /// T811's reply on another board, which became an Asks-you question.
    const T811: &str = "Nothing more can be checked before the 05:00 UTC run. Trigger `t_b3b364` wakes this terminal at 06:30 UTC. Then I'll check pipeline 16 with `az`, attach the results, and draft the reply for Steve's thread. I'll show you the exact text and won't send it until you approve.";

    #[test]
    fn the_wake_time_wins_over_the_run_it_waits_for() {
        let now = ts("2026-11-24T00:23:00Z");
        let u = until(T811, now).expect("it waits until a time");
        assert_eq!(u.time, "06:30 UTC");
        assert_eq!(u.at, ts("2026-11-24T06:30:00Z"));
        assert!(u.plan.starts_with("check pipeline 16 with `az`"), "{}", u.plan);
    }

    #[test]
    fn waiting_until_a_time_reads_as_one() {
        let now = ts("2026-11-24T00:23:00Z");
        assert_eq!(until("Nothing more until the 05:00 UTC run.", now).unwrap().at, ts("2026-11-24T05:00:00Z"));
        assert_eq!(until("I'll check at 06:30 UTC whether it passed.", now).unwrap().time, "06:30 UTC");
        assert_eq!(until("Waiting until tomorrow 9am, then I'll read the logs.", now).unwrap().time, "tomorrow 9am");
        let u = until("- **Song check:** this terminal picks it back up at 4:15 AM Nov 24, after tonight's scan. It checks how many songs the scan added.", now).unwrap();
        assert_eq!(u.time, "2026-11-24T04:15");
        assert_eq!(u.plan, "It checks how many songs the scan added.");
    }

    #[test]
    fn times_that_are_not_a_wait_are_not_read() {
        let now = ts("2026-11-24T00:23:00Z");
        for p in [
            "The nightly build runs at 05:00 UTC and takes an hour.",
            "I'll show you the exact text and won't send it until you approve.",
            "I fixed the 6:30 AM reminder.",
            "It woke at 06:30 UTC and the build passed.",
        ] {
            assert_eq!(until(p, now), None, "{p}");
        }
    }

    #[test]
    fn the_line_has_nothing_a_shell_runs() {
        assert_eq!(quotable("run `az` on \"16\" with $X "), "run az on '16' with X");
    }
}
