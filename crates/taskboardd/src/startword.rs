//! The owner's word to start a task from a terminal (`tb start T4`). Only a human starts a task: by
//! pressing Start on the board, or by telling an agent to. The board doesn't take the agent's say-so:
//! it reads the prompts the human typed in that terminal (the UserPromptSubmit hook reports each one)
//! and starts the task only when one of them asks for it.
//!
//! This is what keeps an agent from starting work outside work hours, so it errs on the side of no: a
//! prompt is the word only when a clause of it asks for the start now ("start T8", "queue it", "kick
//! off T8"), not when it asks about one, tells of one, puts one off or sets a condition on one ("why
//! did T8 start failing?", "T8 started", "start T8 tomorrow", "start T8 once T7 lands", "start T8?").
//! Text the owner pasted or quoted (code, quotes, a log, the line after "says:") is no one's word.
//! The latest prompt that speaks of a task's start decides: "don't start T8" after "start T8" takes it
//! back.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::app::App;
use crate::p;
use crate::util::*;

/// `session_events.data` on a prompt the board sent (`[task-board:T4] …`), which is no one's word.
pub const BOARD_PROMPT: &str = "board";

/// `session_events.kind` for a task the terminal's conversation made (`tb task new`); `data` is its id.
pub const MADE: &str = "added";

/// Any board marker (`[task-board:T4]`, `[task-board:G2]`, `[task-board:J7]`, …): the prompt is the board's.
pub static MARKER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[task-board:[^\]\n]*\]").unwrap());

/// Where an ask can begin: the start of a sentence or of a clause.
const EDGE: &str = r"(?:^|[:,(]|\b(?:and|then|but|so|also)\b)";
/// What may come before the start word in an ask ("ok,", "please", "go ahead and", "can you").
const LEAD: &str = r"(?:(?:ok(?:ay)?|alright|all\s+right|yes|yeah|yep|sure|great|cool|perfect|thanks|please|pls|now|then|so|also|just|go|go\s+ahead(?:\s+and)?|you\s+can|you\s+may|can\s+you|could\s+you|would\s+you|will\s+you|i\s+want\s+you\s+to|i[’']?d\s+like\s+you\s+to|let[’']?s|let\s+us|feel\s+free\s+to|time\s+to)\s*,?\s+)*";
/// The tasks an ask names: "T4", "task T4", "T4, T5 and T6".
const NAMED: &str = r"(?:task\s+)?[Tt]\d+(?:\s*,?\s*(?:and\s+|&\s*)?(?:task\s+)?[Tt]\d+)*";
/// The task an ask doesn't name: "it", "this", "the new task", "them".
const UNNAMED: &str = r"(?:it|this|that|them|these|those|both|(?:the|this|that|these|those|my|your|our)\s+(?:new\s+)?(?:one|ones|task|tasks))";
/// What may follow the task in an ask. Anything else ("start T8 failed", "start T8 on Monday", "start
/// T8 when T7 lands") is no ask; a question mark, a time or a condition is handled by the sentence.
const TAIL: &str = r"(?:\s*$|\s*[:,)]|\s+(?:and|then|now|please|pls|too|again|up|off|asap|for\s+me|right\s+away|right\s+now)\b)";
/// The start words.
const VERB: &str = r"(?:start|queue|begin|launch|kick\s+off)(?:\s+up)?(?:\s+(?:work(?:ing)?\s+)?on)?";

/// A start word at the start of a clause, on a task, with nothing after the task that makes it a story.
static ASK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i){EDGE}\s*{LEAD}(?:\b{VERB}\s+(?:(?P<named>{NAMED})|(?P<unnamed>{UNNAMED}))|\bkick\s+(?:(?P<named2>{NAMED})|(?P<unnamed2>{UNNAMED}))\s+off){TAIL}"
    ))
    .unwrap()
});
/// A start word on a task anywhere: an ask, or (when it's no ask) a mention that takes the start back.
static MENTION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)(?:\b{VERB}\s+(?:(?P<named>{NAMED})|(?P<unnamed>{UNNAMED}))|\bkick\s+(?:(?P<named2>{NAMED})|(?P<unnamed2>{UNNAMED}))\s+off)\b"
    ))
    .unwrap()
});
static REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[Tt](\d+)\b").unwrap());
/// A sentence and what ends it.
static SENTENCE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?P<s>[^.!?;\n]*)(?P<end>[.!?;\n]*)").unwrap());
/// A word that says no ("don't", "never", "do not, under any circumstances,").
static NEG_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:not|never|no|nobody|nothing|none|nor|neither|without|cannot|dont|wont|cant|shouldnt|mustnt|avoid|refrain|instead\s+of|rather\s+than|except|hold\s+off|forget|skip|cancel|nope|nah)\b|n[’']t\b").unwrap()
});
/// "No" that doesn't say no to anything.
static NOT_NEG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bno\s+(?:problem|worries|prob)\b").unwrap());
/// A condition anywhere in the prompt ("start T8. Do it once T7 lands."): it's for later, or for when
/// something else happens.
static CONDITION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:when|whenever|once|if|unless|after|until|till|til|before|provided|providing|assuming|soon|long\s+as|in\s+case|depending|wait|waiting)\b").unwrap()
});
/// A time anywhere in the prompt: whatever start it asks for, it isn't for now.
static TIME_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:until|till|later|tomorrow|tmrw|tmr|tonight|today|overnight|weekend|morning|afternoon|evening|midnight|noon|",
        r"monday|tuesday|wednesday|thursday|friday|saturday|sunday|mon|tues?|wed|thu|thurs?|fri|sat|",
        r"january|february|march|april|june|july|august|september|october|november|december|jan|feb|mar|apr|jun|jul|aug|sept?|oct|nov|dec|",
        r"next\s+(?:week|month|time|sprint|year)|this\s+(?:week|weekend|month|evening|afternoon|morning)|end\s+of\s+(?:the\s+)?(?:day|week)|eod|eow|",
        r"wait|waiting|hold\s+(?:off|on)|whenever|eventually|soon|afterwards?|yet|schedule[ds]?|o[’']?clock|hours?|minutes?|mins?|days?|weeks?|months?|",
        r"in\s+(?:a|an|one|two|three|four|five|few|couple|\d+)\b|at\s+\d|\d{1,2}\s*[ap]\.?m\b|[ap]\.m\.|\d{1,2}:\d{2}|\d{1,2}/\d{1,2})"
    ))
    .unwrap()
});
/// A word on its own that takes back what came before ("wait", "never mind", "hold off").
static HALT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:wait|hold\s+(?:off|on|up)|never\s*mind|nvm|scratch\s+that|cancel\s+(?:that|it|this|them)|forget\s+(?:it|that|this)|not\s+yet|stop|pause|don[’']?t\s+(?:do\s+(?:it|that)|bother)|actually\s+no)\b").unwrap()
});
/// A word that holds a task off when a sentence names it outside a start ("hold off on T8", "T8 can wait").
static HOLD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:not|never|no|don[’']?t|do\s+not|wait|hold|pause|stop|later|leave|alone|yet|skip|cancel|forget|instead)\b|n[’']t\b").unwrap()
});
/// The prompt asks for a new task, the one an unnamed ask ("make a task and queue it") can mean.
static MAKE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:make|create|add|file|open|write|set\s+up|log|draft|new)\b[^.!?;\n]*\b(?:task|ticket|card|one)s?\b").unwrap()
});

/// Fenced and inline code.
static CODE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)```.*?(?:```|$)|`[^`\n]*`?").unwrap());
/// Quoted text, closed or left open to the end of the line.
static QUOTE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)"[^"\n]*(?:"|$)|“[^”\n]*(?:”|$)|‘[^’\n]*(?:’|$)|«[^»\n]*(?:»|$)|(?:^|[\s(\[{:,])'[^'\n]*(?:'|$)"#).unwrap()
});
/// What comes after this on the line is someone else's words ("the log says: …").
static SAYS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:says?|said|saying|wrote|writes|reads?|shows?|showed|shown|prints?|printed|outputs?|logs?|logged|errors?|replied|reply|replies|message|quoted?|quotes|asked|asks|tells?|told|claims?|returns?|returned|got|gives|gave|stdout|stderr|response|text|comment|commented|wants?|wanted|posted|typed)\b\s*:").unwrap()
});
/// A line that reads as pasted (a log line, a quote, a prompt, a diff, a list).
static PASTED_LINE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:\s|[>$#|\[{<+*\-•]|\d{1,4}[:\-/.)\]])").unwrap());

/// One ask for a start in a prompt: for the tasks it names, or for a task it doesn't ("queue it").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Named(Vec<i64>),
    Unnamed,
}

/// What a prompt says about starting tasks: the asks it makes now, and the starts it takes back, puts
/// off, asks about or sets a condition on (`Unnamed` there takes back every earlier ask).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reading {
    pub asks: Vec<Ask>,
    pub held: Vec<Ask>,
    /// The prompt asks for a new task: what an unnamed ask can mean.
    pub makes: bool,
}

impl Reading {
    fn holds(&self, id: i64) -> bool {
        self.held.iter().any(|a| match a {
            Ask::Named(ids) => ids.contains(&id),
            Ask::Unnamed => true,
        })
    }
    fn names(&self, id: i64) -> bool {
        self.asks.iter().any(|a| matches!(a, Ask::Named(ids) if ids.contains(&id)))
    }
    fn unnamed(&self) -> bool {
        self.asks.contains(&Ask::Unnamed)
    }
}

/// The owner's own words in `text`: code, quotes, the rest of a line after "says:", the lines after a
/// line that ends in a colon (up to a blank line) and lines that read as pasted all come out.
pub fn owners_text(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let text = CODE_RE.replace_all(&text, "\n");
    // A quote is no clause edge: the words around it stay one sentence (and keep their "don't").
    let text = QUOTE_RE.replace_all(&text, " _ ");
    let mut out = vec![];
    let mut pasted = false;
    for line in text.split('\n') {
        if pasted {
            pasted = !line.trim().is_empty();
            continue;
        }
        if line.trim().is_empty() || PASTED_LINE_RE.is_match(line) {
            out.push(String::new());
            continue;
        }
        let mut own = line;
        if let Some(m) = SAYS_RE.find(line) {
            own = &line[..m.start()];
            pasted = line[m.end()..].trim().is_empty();
        } else if line.trim_end().ends_with(':') {
            pasted = true;
        }
        out.push(own.to_string());
    }
    out.join("\n")
}

/// The task spans and asks of one mention.
fn mention_of<'h>(c: &regex::Captures<'h>) -> Option<(regex::Match<'h>, Ask)> {
    let (named, unnamed) = (c.name("named").or(c.name("named2")), c.name("unnamed").or(c.name("unnamed2")));
    match (named, unnamed) {
        (Some(n), _) => Some((n, Ask::Named(named_tasks(n.as_str())))),
        (None, Some(u)) => Some((u, Ask::Unnamed)),
        _ => None,
    }
}

/// The tasks a stretch of a sentence points at: by name, or by a start word on an unnamed one.
fn refs_in(s: &str) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = REF_RE.find_iter(s).map(|m| (m.start(), m.end())).collect();
    out.extend(MENTION_RE.captures_iter(s).filter_map(|c| c.name("unnamed").or(c.name("unnamed2")).map(|u| (u.start(), u.end()))));
    out.sort();
    out
}

/// Whether a "no" in sentence `s` falls on the task at `span`: a no belongs to the next task the
/// sentence points at after it ("don't start T4, start T5"), or, with none after it, to every task
/// before it ("start T8, never mind").
fn said_no(s: &str, span: (usize, usize)) -> bool {
    let refs = refs_in(s);
    NEG_RE.find_iter(s).filter(|n| !NOT_NEG_RE.find_iter(s).any(|x| x.start() <= n.start() && n.end() <= x.end())).any(|n| {
        match refs.iter().find(|r| r.0 >= n.end()) {
            Some(next) => next.0 == span.0,
            None => span.1 <= n.start(),
        }
    })
}

/// What `text` says about starting tasks.
pub fn read(text: &str) -> Reading {
    let own = owners_text(text);
    let timed = TIME_RE.is_match(&own) || CONDITION_RE.is_match(&own);
    let mut r = Reading { makes: MAKE_RE.is_match(&own), ..Reading::default() };
    for c in SENTENCE_RE.captures_iter(&own) {
        let s = c.name("s").map(|m| m.as_str()).unwrap_or("");
        if s.trim().is_empty() {
            continue;
        }
        let asked = c.name("end").map(|m| m.as_str().contains('?')).unwrap_or(false);
        // Where a clean ask lands: the task span of each ASK_RE match.
        let mut ask_spans = vec![];
        let mut at = 0;
        while let Some(m) = ASK_RE.captures_at(s, at) {
            let Some((task, _)) = mention_of(&m) else { break };
            ask_spans.push((task.start(), task.end()));
            at = task.end();
        }
        let mut in_mentions = vec![];
        for m in MENTION_RE.captures_iter(s) {
            let Some((task, ask)) = mention_of(&m) else { continue };
            let span = (task.start(), task.end());
            in_mentions.push(span);
            let clean = ask_spans.contains(&span) && !asked && !timed && !said_no(s, span);
            if clean { r.asks.push(ask) } else { r.held.push(ask) }
        }
        // A task named outside a start, in a sentence that holds it off ("hold off on T8", "T8 can wait").
        let loose: Vec<i64> = REF_RE
            .captures_iter(s)
            .filter(|c| {
                let m = c.get(0).unwrap();
                !in_mentions.iter().any(|(a, b)| *a <= m.start() && m.end() <= *b)
            })
            .filter_map(|c| c[1].parse().ok())
            .collect();
        if !loose.is_empty() && HOLD_RE.is_match(s) {
            r.held.push(Ask::Named(loose));
        }
        if REF_RE.find(s).is_none() && HALT_RE.is_match(s) {
            r.held.push(Ask::Unnamed);
        }
    }
    r
}

/// The asks for a start in `text`.
pub fn asks(text: &str) -> Vec<Ask> {
    read(text).asks
}

/// Whether `text` asks for something to start or be queued.
pub fn says_start(text: &str) -> bool {
    !asks(text).is_empty()
}

/// The tasks `text` names (T4 → 4).
pub fn named_tasks(text: &str) -> Vec<i64> {
    REF_RE.captures_iter(text).filter_map(|c| c[1].parse().ok()).collect()
}

/// One prompt of the conversation, for [`word_in`].
pub struct Prompt {
    pub text: String,
    /// The first task the conversation made after this prompt.
    pub first_made: Option<i64>,
}

/// Which prompt, newest first, is the word to start task `id`. The latest prompt that speaks of the
/// task's start decides: if it takes the start back, puts it off or asks about it, there's no word.
/// An unnamed ask ("make a task for it and queue it") is the word only in the latest prompt, when that
/// prompt asks for a new task and `id` is the first task the conversation made after it.
pub fn word_in(prompts: &[Prompt], id: i64) -> Option<usize> {
    for (i, p) in prompts.iter().enumerate() {
        if MARKER_RE.is_match(&p.text) {
            continue;
        }
        let r = read(&p.text);
        if r.holds(id) {
            return None;
        }
        if r.names(id) || (i == 0 && r.unnamed() && r.makes && p.first_made == Some(id)) {
            return Some(i);
        }
    }
    None
}

/// Notes on terminal `sid` that its conversation made task `id`: the task an unnamed "queue it" in the
/// prompt before can mean.
pub fn note_made(app: &App, sid: &str, id: i64) -> Result<()> {
    crate::board::session_event_with(app, sid, MADE, &format!("Added {}", rf("task", id)), None, Some(&id.to_string()))
}

/// The prompt a human typed in terminal `sid`, since its Claude conversation began, that asks for task
/// `id` to start, if there's one and no later prompt takes it back.
pub fn owners_word(app: &App, sid: &str, id: i64) -> Result<Option<String>> {
    if sid.is_empty() {
        return Ok(None);
    }
    let rows = app.db.q(
        "SELECT id, text FROM session_events WHERE session_id = ? AND kind = 'prompt' AND COALESCE(data, '') != ?
           AND id > COALESCE((SELECT MAX(id) FROM session_events WHERE session_id = ? AND kind = 'start'), 0)
         ORDER BY id DESC",
        p![sid, BOARD_PROMPT, sid],
    )?;
    let first_made = match rows.first() {
        Some(latest) => app
            .db
            .q1("SELECT data FROM session_events WHERE session_id = ? AND kind = ? AND id > ? ORDER BY id LIMIT 1", p![sid, MADE, latest.id()])?
            .and_then(|r| r.st("data").parse().ok()),
        None => None,
    };
    let prompts: Vec<Prompt> =
        rows.iter().enumerate().map(|(i, r)| Prompt { text: r.st("text"), first_made: if i == 0 { first_made } else { None } }).collect();
    Ok(word_in(&prompts, id).map(|i| prompts[i].text.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_is_asked_for_unless_it_says_not_to() {
        for yes in [
            "create a new task and queue it",
            "ok, start T4",
            "kick it off",
            "kick off T8",
            "kick T8 off",
            "Go ahead and launch T7 now",
            "start T8",
            "Start T8.",
            "queue it",
            "please queue T8",
            "start T8 please",
            "could you queue up T8 for me",
            "let's begin T8",
            "start working on T8",
            "start the new task",
            "yes, start it",
            "looks good. queue it",
            "(start T8)",
            "start T8 right now",
            "no problem, start T8",
            "start T8!",
            "the tests pass. start T8",
        ] {
            assert!(says_start(yes), "{yes}");
        }
        for no in [
            "make a task for it but don't start it",
            "add T4, do not queue it yet",
            "no need to start T4",
            "fix the restart bug",
            "what's on the board?",
            "why did T8 start failing yesterday?",
            "T8 started an hour ago",
            "started",
            "starting",
            "queued",
            "beginning",
            "it's starting now",
            "the dev server",
            "start the dev server and check the login page",
            "start the dev server",
            "kick off a rebuild",
            "did you start T8?",
            "should I start T8?",
            "when will you start T8?",
            "who started T8",
            "the server won't start",
            "you can't start T8 until T7 lands",
            "you cannot start T8",
            "Start T8 failed yesterday",
            "start T8 is broken",
            "start this weekend",
            "the log says \"start T8\"",
            "how do I start T8",
            "I started T8",
            "T8 is queued",
            "restart T8",
            "requeue it",
            "startup is slow",
            "launching T8 broke prod",
            "no, start it",
        ] {
            assert!(!says_start(no), "{no}");
        }
    }

    #[test]
    fn a_no_before_a_comma_still_says_no() {
        for no in [
            "never, ever, start T8",
            "do not, under any circumstances, start T8",
            "don't, please, start T8",
            "no, start T8",
            "not now, start T8",
            "please don't, ok, queue it",
            "start T8, never mind",
            "start T8, or not",
            "Never start T8",
        ] {
            assert!(!says_start(no), "{no}");
            assert!(!read(no).held.is_empty(), "{no} takes the start back");
        }
    }

    #[test]
    fn a_start_for_later_or_on_a_condition_is_no_ask() {
        for no in [
            "wait until 6am, then start T8",
            "wait until 6 a.m., then start T8",
            "start T8 on Monday",
            "start T8 in two weeks",
            "start T8 in 5 minutes",
            "start T8 when T7 lands",
            "start T8 once I say so",
            "once T7 is merged, start T8",
            "start T8 after lunch",
            "after lunch, start T8",
            "start T8 tomorrow",
            "start T8 tonight",
            "start T8 today",
            "start T8 at 9",
            "start T8 at 9:30",
            "start T8 at 6pm",
            "start T8 if the tests pass",
            "start T8 unless T7 breaks",
            "start T8 before you go",
            "start T8 later",
            "start T8 next week",
            "start T8 this weekend",
            "start T8 first thing in the morning",
            "start T8 as soon as T7 is done",
            "start T8 whenever",
            "start T8 on the 12th",
            "start T8 with the new branch",
            "hold off for now. start T8 later",
            "queue it for 10/12",
            "Start T8. Do it after the deploy.",
            "start T8 when you're ready",
        ] {
            assert!(!says_start(no), "{no}");
            assert!(!read(no).held.is_empty(), "{no} holds the start");
        }
    }

    #[test]
    fn a_question_is_no_ask() {
        for no in ["start T8?", "so, start T8?", "can you start T8?", "ok, queue it?", "start T8 now?", "start T8?!", "should we start T8 and T9?"] {
            assert!(!says_start(no), "{no}");
            assert!(!read(no).held.is_empty(), "{no}");
        }
    }

    #[test]
    fn quoted_or_pasted_text_is_no_ask() {
        for no in [
            "the log says: start T8.",
            "The log says: start T8",
            "it printed:\nstart T8\nstart T9",
            "here's what I see:\nstart T8",
            "here's the log\n  start T8",
            "here's the log\n\tstart T8",
            "> start T8",
            "$ tb start T8",
            "[12:01] start T8",
            "12:01 start T8",
            "- start T8",
            "```\nstart T8\n```",
            "run `start T8`",
            "he wrote “start T8” in the chat",
            "it said 'start T8' somewhere",
            "the message: start T8",
            "Alex asked: start T8",
            "the comment reads \"start T8",
            "```\nstart T8",
        ] {
            assert!(!says_start(no), "{no:?}");
        }
        // The owner's own line around a paste still counts.
        assert!(says_start("start T8\n\nthe log says: whatever"));
        assert!(says_start("here's the log:\n  ERROR x\n\nok, start T8"));
    }

    #[test]
    fn each_ask_names_its_own_tasks() {
        assert_eq!(asks("start T4 and T5"), vec![Ask::Named(vec![4, 5])]);
        assert_eq!(asks("start T4, T5 and task T6"), vec![Ask::Named(vec![4, 5, 6])]);
        assert_eq!(asks("start T4 and queue T5"), vec![Ask::Named(vec![4]), Ask::Named(vec![5])]);
        assert_eq!(asks("don't start T4, start T5"), vec![Ask::Named(vec![5])]);
        assert_eq!(read("don't start T4, start T5").held, vec![Ask::Named(vec![4])]);
        assert_eq!(asks("start T4 and don't start T5"), vec![Ask::Named(vec![4])]);
        assert_eq!(asks("make a task and queue it"), vec![Ask::Unnamed]);
        assert_eq!(asks("start T8, not T9"), vec![Ask::Named(vec![8])]);
        assert_eq!(read("start T8, not T9").held, vec![Ask::Named(vec![9])]);
    }

    #[test]
    fn a_task_held_off_without_a_start_word_is_held() {
        for (said, id) in [("hold off on T8", 8), ("T8 can wait", 8), ("leave T8 alone", 8), ("not T8 yet", 8), ("pause T8", 8), ("skip T8", 8)] {
            assert!(read(said).holds(id), "{said}");
        }
        for said in ["wait", "wait, what's this error?", "never mind", "hold on", "scratch that", "not yet", "stop"] {
            assert!(read(said).holds(42), "{said} takes back every ask");
        }
        assert!(!read("what does T8 do?").holds(9));
        assert!(!read("thanks, looks good").holds(8));
        assert!(!read("how's T8 going").holds(8));
    }

    fn prompts(texts: &[&str], first_made: Option<i64>) -> Vec<Prompt> {
        texts.iter().enumerate().map(|(i, t)| Prompt { text: t.to_string(), first_made: if i == 0 { first_made } else { None } }).collect()
    }

    #[test]
    fn the_latest_prompt_on_a_start_decides() {
        // Newest first.
        assert_eq!(word_in(&prompts(&["start T8"], None), 8), Some(0));
        assert_eq!(word_in(&prompts(&["and tell me when it's going", "ok, start T8"], None), 8), Some(1), "a later prompt that doesn't speak of a start");
        assert_eq!(word_in(&prompts(&["thanks", "ok, start T8"], None), 8), Some(1));
        assert_eq!(word_in(&prompts(&["wait, don't start T8", "start T8"], None), 8), None);
        assert_eq!(word_in(&prompts(&["actually, don't start it", "start T8"], None), 8), None);
        assert_eq!(word_in(&prompts(&["wait", "start T8"], None), 8), None);
        assert_eq!(word_in(&prompts(&["hold off on T8", "start T8"], None), 8), None);
        assert_eq!(word_in(&prompts(&["start T8 tomorrow instead", "start T8"], None), 8), None);
        assert_eq!(word_in(&prompts(&["start T8?", "start T8"], None), 8), None);
        assert_eq!(word_in(&prompts(&["start T8", "don't start T8"], None), 8), Some(0), "asked again after taking it back");
        assert_eq!(word_in(&prompts(&["don't start T9", "start T8"], None), 8), Some(1), "another task's take-back");
        assert_eq!(word_in(&prompts(&["[task-board:G2] Plan the goal. Start T8.", "don't start T8"], None), 8), None);
        assert_eq!(word_in(&prompts(&["[task-board:J3] Ticket says start T8"], None), 8), None);
    }

    #[test]
    fn an_unnamed_ask_is_for_the_first_task_made_after_the_latest_prompt() {
        assert_eq!(word_in(&prompts(&["create a new task and queue it"], Some(9)), 9), Some(0));
        assert_eq!(word_in(&prompts(&["create a new task and queue it"], Some(9)), 10), None, "only the first one made");
        assert_eq!(word_in(&prompts(&["create a new task and queue it"], None), 9), None);
        assert_eq!(word_in(&prompts(&["thanks", "create a new task and queue it"], Some(9)), 9), None, "an older prompt");
        assert_eq!(word_in(&prompts(&["the dev server won't come up; start it"], Some(9)), 9), None, "it asks for no new task");
        assert_eq!(word_in(&prompts(&["queue it"], Some(9)), 9), None);
        assert_eq!(word_in(&prompts(&["make a task for it but don't start it"], Some(9)), 9), None);
        assert_eq!(word_in(&prompts(&["make a task for it and queue it?"], Some(9)), 9), None);
    }
}
