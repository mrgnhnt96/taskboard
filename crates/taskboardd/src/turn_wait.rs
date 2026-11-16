//! A turn that ends saying the task waits on other board tasks ("T12 is now waiting for T15 and T16;
//! once both are in, I'll…") without running `tb wait-for`. Nothing records that wait, so nothing brings
//! the conversation back when they finish, and the idle terminal would sit on the owner as "Waiting for
//! you". The Stop hook sends the agent back once with the exact `tb wait-for` line; when its turn ends
//! again still saying it waits, the board records the wait itself, as `tb wait-for` would.

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
}
