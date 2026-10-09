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

/// A typed prompt this long that ends in "…" was clipped on its way to the board (the hook keeps 8000
/// characters, the board [`crate::board::PROMPT_FULL_KEEP`]): its unread end may take a start back.
const PROMPT_CUT_AT: usize = 4000;

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
/// The task an ask doesn't name: "it", "this", "the new task", "them", "both of them".
const UNNAMED: &str = concat!(
    r"(?:(?:both|all|each)\s+of\s+(?:them|these|those|the\s+(?:new\s+)?(?:tasks|ones))|them\s+(?:all|both)|",
    r"all\s+(?:the\s+|these\s+|those\s+)?(?:new\s+)?(?:tasks|ones)|all\s+(?:two|three|four|five|six)|",
    r"(?:the|these|those|my|your|our)\s+(?:two|three|four|five|six)\s+(?:new\s+)?(?:tasks|ones)|",
    r"(?:the|this|that|these|those|my|your|our)\s+(?:new\s+)?(?:one|ones|task|tasks)|it|this|that|them|these|those|both)"
);
/// An unnamed ask that means more than one task ("them", "both", "these", "the new tasks").
static PLURAL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:them|these|those|both|all|ones|tasks)\b").unwrap());
/// A prompt that's nothing but an unnamed ask ("queue it", "ok, start them", "great, kick it off"): its
/// "it" can only be what the conversation just made.
static BARE_ASK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)^(?:(?:ok(?:ay)?|alright|all\s+right|yes|yeah|yep|yup|sure|great|cool|nice|perfect|awesome|good|thanks|thank\s+you|ty|please|pls|looks\s+good|lgtm|sounds\s+good|go\s+ahead(?:\s+and)?|now|then|so|and|just|you\s+can|can\s+you|could\s+you|let[’']?s|that[’']?s\s+(?:fine|great|good|perfect))[\s,.!]*)*(?:{VERB}\s+(?:{UNNAMED})|kick\s+(?:{UNNAMED})\s+off)(?:\s+(?:up|off|now|please|pls|too|for\s+me|right\s+away|right\s+now|asap))*[\s.!]*(?:(?:thanks|thank\s+you|ty|please|pls)[\s.!]*)?$"
    ))
    .unwrap()
});
/// What may follow the task in an ask. Anything else ("start T8 failed", "start T8 on Monday", "start
/// T8 when T7 lands") is no ask; a question mark, a time or a condition is handled by the sentence.
const TAIL: &str = r"(?:\s*$|\s*[:,)]|\s+(?:and|then|now|please|pls|too|again|up|off|asap|instead|for\s+me|right\s+away|right\s+now)\b)";
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
    Regex::new(concat!(
        r"(?i)\b(?:when|whenever|once|if|unless|after|until|till|til|before|provided|providing|assuming|soon|long\s+as|in\s+case|depending|wait|waiting|",
        r"the\s+(?:moment|minute|second|instant|day|time)|by\s+the\s+time|on\s+(?:my|your)\s+(?:go|signal|mark|word|say))\b"
    ))
    .unwrap()
});
/// A time anywhere in the prompt: whatever start it asks for, it isn't for now. Whole words only:
/// "never mind" has no "min" in it, and "monitor" no "mon".
static TIME_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:until|till|later|tomorrow|tmrw|tmr|tonight|today|overnight|weekends?|mornings?|afternoons?|evenings?|nights|midnight|noon|",
        r"monday|tuesday|wednesday|thursday|friday|saturday|sunday|mon|tues?|wed|thu|thurs?|fri|sat|",
        r"january|february|march|april|june|july|august|september|october|november|december|jan|feb|mar|apr|jun|jul|aug|sept?|oct|nov|dec|",
        r"next\s+(?:week|month|time|sprint|year)|this\s+(?:week|weekend|month|evening|afternoon|morning)|end\s+of\s+(?:the\s+)?(?:day|week)|eod|eow|",
        r"wait|waiting|hold\s+(?:off|on)|whenever|eventually|soon|afterwards?|yet|schedule[ds]?|o[’']?clock|hours?|minutes?|mins?|days?|weeks?|months?|",
        r"first\s+thing|lunch(?:time)?|dinner|bedtime|sometime|someday|shortly|momentarily|at\s+some\s+point|half\s+past|quarter\s+(?:past|to)|",
        r"(?:at|by|around|about|past)\s+(?:one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|dawn|dusk|night)|\d{1,2}\s*ish|",
        r"in\s+(?:a|an|one|two|three|four|five|few|couple|\d+)|at\s+\d+|\d{1,2}\s*[ap]\.?m|\d{1,2}:\d{2}|\d{1,2}/\d{1,2})\b|\b[ap]\.m\."
    ))
    .unwrap()
});
/// Words that take back what came before them ("wait", "never mind", "jk", "on second thought, don't"),
/// in the prompt or in an earlier one, when no task comes after them in the sentence.
static HALT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:wait|hold\s+(?:off|on|up)|never\s*mind|nvm|scratch\s+that|cancel\s+(?:that|it|this|them|these|those)|",
        r"forget\s+(?:it|that|this|them|about\s+(?:it|that|this|them)|(?:what\s+)?i\s+said)|not\s+(?:yet|now|today|right\s+now|so\s+fast)|",
        r"don[’']?t\s+(?:do\s+(?:it|that|this)|bother|start|queue|begin|launch|kick)|do\s+not\s+(?:do\s+(?:it|that|this)|start|queue|begin|launch|kick)|",
        r"actually\s+no|jk|j/k|just\s+(?:kidding|joking)|kidding|joking|on\s+second\s+thoughts?|second\s+thoughts?|",
        r"change[ds]?\s+(?:of\s+)?(?:my\s+)?mind|changing\s+my\s+mind|change\s+of\s+plans?|take\s+(?:it|that|this)\s+back|belay\s+that|",
        r"ignore\s+(?:that|this|it|what\s+i\s+said|my\s+last)|disregard|undo\s+(?:that|it|this)|let[’']?s\s+not|rather\s+not|better\s+not|or\s+not|no\s+longer)\b"
    ))
    .unwrap()
});
/// A sentence that ends on a no ("on second thought, don't", "please don't", "I'd rather you not").
static ENDS_NO_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:don[’']?t|do\s+not|not|never)\s*(?:please|pls|yet|now|though|tho|after\s+all|anymore|any\s+more)?\s*[,!]*\s*$").unwrap()
});
/// A sentence that starts with a no ("no.", "nope", "no, don't", "nah").
static STARTS_NO_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s,]*(?:(?:oh|ah|um+|uh+|hm+|actually|wait|ok(?:ay)?|well|so|and|but)\b[\s,]*)*(?P<no>no+|nope|nah|naw|nay|negative)\b").unwrap()
});
/// Words that stop things, which take back what came before only in a sentence that names no task
/// ("stop", but not "start T8 and stop the dev server").
static STOP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:stop|pause|halt|abort)\b").unwrap());
/// An ask that replaces the ones before it ("start T9 instead").
static INSTEAD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:instead|rather)\b").unwrap());
/// A no that falls only on the very next task the sentence names ("instead of T8, start T9").
static NO_OF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:instead\s+of|rather\s+than|except)$").unwrap());
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

/// One ask for a start in a prompt: for the tasks it names, for a task it doesn't name ("queue it"),
/// or for tasks it doesn't name ("queue them").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Named(Vec<i64>),
    Unnamed,
    Them,
}

/// One thing a prompt says about a start, in the order it says them: an ask, or a take-back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Ask(Ask),
    Hold(Ask),
}

/// What a prompt says about starting tasks: the asks it makes now, and the starts it takes back, puts
/// off, asks about or sets a condition on (`Unnamed` there takes back every earlier ask).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reading {
    pub asks: Vec<Ask>,
    pub held: Vec<Ask>,
    /// The asks and take-backs in the order the prompt says them: the last one on a task decides
    /// ("start T8. jk" takes it back, "never mind. Start T8." asks again).
    pub steps: Vec<Step>,
    /// The prompt asks for a new task: what an unnamed ask can mean.
    pub makes: bool,
}

impl Reading {
    /// Whether the prompt is the word to start task `id` (`Some(true)`), takes it back (`Some(false)`),
    /// or says nothing on it. `unnamed` says whether an unnamed ask means `id`.
    fn word_for(&self, id: i64, unnamed: impl Fn(&Ask) -> bool) -> Option<bool> {
        self.steps.iter().rev().find_map(|s| match s {
            Step::Hold(Ask::Named(ids)) => ids.contains(&id).then_some(false),
            Step::Hold(_) => Some(false),
            Step::Ask(Ask::Named(ids)) => ids.contains(&id).then_some(true),
            Step::Ask(a) => unnamed(a).then_some(true),
        })
    }
    /// Whether the prompt, on its own, leaves task `id` held: the last it says on the task takes it back.
    #[cfg(test)]
    fn holds(&self, id: i64) -> bool {
        self.word_for(id, |_| false) == Some(false)
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
    // After a line that hands over a paste ("CI failed with:"), the paste: from the next line that
    // isn't blank to the next blank line.
    let (mut pasted, mut lead) = (false, false);
    for line in text.split('\n') {
        if pasted {
            if line.trim().is_empty() && lead {
                out.push(String::new());
                continue;
            }
            lead = false;
            pasted = !line.trim().is_empty();
            out.push(String::new());
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
        lead = pasted;
        out.push(own.to_string());
    }
    out.join("\n")
}

/// The task spans and asks of one mention.
fn mention_of<'h>(c: &regex::Captures<'h>) -> Option<(regex::Match<'h>, Ask)> {
    let (named, unnamed) = (c.name("named").or(c.name("named2")), c.name("unnamed").or(c.name("unnamed2")));
    match (named, unnamed) {
        (Some(n), _) => Some((n, Ask::Named(named_tasks(n.as_str())))),
        (None, Some(u)) => Some((u, if PLURAL_RE.is_match(u.as_str()) { Ask::Them } else { Ask::Unnamed })),
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

/// The task spans of the start words in a sentence.
fn mentions_in(s: &str) -> Vec<(usize, usize)> {
    MENTION_RE.captures_iter(s).filter_map(|c| mention_of(&c).map(|(t, _)| (t.start(), t.end()))).collect()
}

/// The words in `s` that say no, less the "no" of "no problem".
fn negs_in(s: &str) -> Vec<regex::Match<'_>> {
    let not: Vec<_> = NOT_NEG_RE.find_iter(s).collect();
    NEG_RE.find_iter(s).filter(|n| !not.iter().any(|x| x.start() <= n.start() && n.end() <= x.end())).collect()
}

/// Whether a "no" in sentence `s` falls on the task at `span`: a no belongs to the next task the
/// sentence points at after it ("don't start T4, start T5"), or, with none after it, to every task
/// before it ("start T8, never mind"). When the next task is only named, not started ("don't (like T9),
/// start T8"), the no falls on the next start too: an aside doesn't take the no off it.
fn said_no(s: &str, span: (usize, usize)) -> bool {
    let (refs, mentions) = (refs_in(s), mentions_in(s));
    negs_in(s).iter().any(|n| match refs.iter().find(|r| r.0 >= n.end()) {
        Some(next) => {
            next.0 == span.0
                || (!NO_OF_RE.is_match(n.as_str())
                    && !mentions.contains(next)
                    && mentions.iter().find(|m| m.0 >= next.1).is_some_and(|m| m.0 == span.0))
        }
        None => span.1 <= n.start(),
    })
}

/// Whether sentence `s` asks for a new task (and doesn't say not to: "don't make a task, just queue it").
fn makes_in(s: &str) -> bool {
    MAKE_RE.find(s).is_some_and(|m| negs_in(&s[..m.start()]).is_empty())
}

/// Where in sentence `s` it takes back every start before it, if it does: a take-back word with no task
/// after it in the sentence ("start T8, scratch that", "jk", "no.", "on second thought, don't").
fn halt_in(s: &str) -> Option<usize> {
    let refs = refs_in(s);
    let last = |at: usize| !refs.iter().any(|r| r.0 >= at);
    let mut at: Vec<usize> = HALT_RE.find_iter(s).filter(|m| last(m.end())).map(|m| m.start()).collect();
    at.extend(ENDS_NO_RE.find(s).filter(|m| last(m.end())).map(|m| m.start()));
    if let Some(no) = STARTS_NO_RE.captures(s).and_then(|c| c.name("no")) {
        let fine = NOT_NEG_RE.find_at(s, no.start()).is_some_and(|x| x.start() == no.start());
        if !fine && last(no.end()) {
            at.push(no.start());
        }
    }
    if refs.is_empty() {
        at.extend(STOP_RE.find(s).map(|m| m.start()));
    }
    at.into_iter().min()
}

/// What `text` says about starting tasks.
pub fn read(text: &str) -> Reading {
    let own = owners_text(text);
    let timed = TIME_RE.is_match(&own) || CONDITION_RE.is_match(&own);
    let mut r = Reading::default();
    // (where it's said, what's said), sorted at the end so the steps run in the prompt's order.
    let mut steps: Vec<(usize, Step)> = vec![];
    for c in SENTENCE_RE.captures_iter(&own) {
        let Some(sm) = c.name("s") else { continue };
        let (base, s) = (sm.start(), sm.as_str());
        if s.trim().is_empty() {
            continue;
        }
        r.makes |= makes_in(s);
        let asked = c.name("end").map(|m| m.as_str().contains('?')).unwrap_or(false);
        // Where a clean ask lands: the task span of each ASK_RE match.
        let mut ask_spans = vec![];
        let mut at = 0;
        while let Some(m) = ASK_RE.captures_at(s, at) {
            let Some((task, _)) = mention_of(&m) else { break };
            ask_spans.push((task.start(), task.end()));
            at = task.end();
        }
        let mentions: Vec<((usize, usize), Ask)> =
            MENTION_RE.captures_iter(s).filter_map(|m| mention_of(&m).map(|(t, a)| ((t.start(), t.end()), a))).collect();
        // "start T9 instead": the asks before this one are off.
        if !mentions.is_empty() && INSTEAD_RE.is_match(s) {
            steps.push((base, Step::Hold(Ask::Unnamed)));
        }
        for (span, ask) in &mentions {
            let clean = ask_spans.contains(span) && !asked && !timed && !said_no(s, *span);
            steps.push((base + span.0, if clean { Step::Ask(ask.clone()) } else { Step::Hold(ask.clone()) }));
        }
        // A task named outside a start, in a sentence that holds it off ("hold off on T8", "T8 can wait").
        let loose: Vec<i64> = REF_RE
            .captures_iter(s)
            .filter(|c| {
                let m = c.get(0).unwrap();
                !mentions.iter().any(|((a, b), _)| *a <= m.start() && m.end() <= *b)
            })
            .filter_map(|c| c[1].parse().ok())
            .collect();
        if !loose.is_empty() && HOLD_RE.is_match(s) {
            steps.push((base + s.len(), Step::Hold(Ask::Named(loose))));
        }
        if let Some(at) = halt_in(s) {
            steps.push((base + at, Step::Hold(Ask::Unnamed)));
        }
    }
    steps.sort_by_key(|(at, _)| *at);
    r.steps = steps.into_iter().map(|(_, s)| s).collect();
    // An ask stands unless something after it in the prompt takes it back ("start T8. jk").
    for (i, step) in r.steps.iter().enumerate() {
        match step {
            Step::Hold(a) => r.held.push(a.clone()),
            Step::Ask(a) => {
                let later = &r.steps[i + 1..];
                let kept = match a {
                    _ if later.iter().any(|s| matches!(s, Step::Hold(h) if !matches!(h, Ask::Named(_)))) => None,
                    Ask::Named(ids) => {
                        let off = |id: &i64| later.iter().any(|s| matches!(s, Step::Hold(Ask::Named(h)) if h.contains(id)));
                        let ids: Vec<i64> = ids.iter().copied().filter(|id| !off(id)).collect();
                        (!ids.is_empty()).then_some(Ask::Named(ids))
                    }
                    a => Some(a.clone()),
                };
                match kept {
                    Some(k) => r.asks.push(k),
                    None => r.held.push(a.clone()),
                }
            }
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
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    pub text: String,
    /// The tasks the conversation made in reply to this prompt (after it, before the next one), in order.
    pub made: Vec<i64>,
    /// Only the start of the prompt reached the board: what it says past that is unknown.
    pub clipped: bool,
    /// The board sent it (`[task-board:…]`): it's no one's word, and no prompt for "queue it" to follow.
    pub board: bool,
}

impl Prompt {
    pub fn new(text: &str) -> Prompt {
        Prompt { text: text.to_string(), ..Prompt::default() }
    }
    fn boards(&self) -> bool {
        self.board || MARKER_RE.is_match(&self.text)
    }
}

/// The tasks an unnamed ask in the latest prompt, `prompts[0]`, can mean. In a prompt that asks for new
/// tasks, the ones made in reply to it: "make a task for it and queue it" means the first, "make tasks
/// for A and B and queue them" every one. In a prompt that's only the ask ("queue it", "ok, start them"),
/// the ones made in reply to the prompt just before, when that one asked for them: "it" only when there's
/// one. A prompt with more in it ("the dev server won't come up; start it") may mean something else.
fn unnamed_tasks(prompts: &[Prompt], r: &Reading, plural: bool) -> Vec<i64> {
    let Some(p) = prompts.first() else { return vec![] };
    if r.makes {
        return if plural { p.made.clone() } else { p.made.first().copied().into_iter().collect() };
    }
    let Some(prev) = prompts.get(1) else { return vec![] };
    if prev.boards() || prev.clipped || !read(&prev.text).makes || !BARE_ASK_RE.is_match(owners_text(&p.text).trim()) {
        return vec![];
    }
    if plural || prev.made.len() == 1 {
        prev.made.clone()
    } else {
        vec![]
    }
}

/// Which prompt, newest first, is the word to start task `id`. The latest prompt that speaks of the
/// task's start decides: if it takes the start back, puts it off or asks about it, there's no word. A
/// prompt that takes back every start without naming one ("nope", "never mind that", "I changed my
/// mind") or asks for another start in place of the earlier ones ("start T9 instead") speaks of them all.
/// An unnamed ask ("queue it", "queue them") is the word only in the latest prompt, for the tasks
/// [`unnamed_tasks`] says it means.
pub fn word_in(prompts: &[Prompt], id: i64) -> Option<usize> {
    for (i, p) in prompts.iter().enumerate() {
        if p.boards() {
            continue;
        }
        // What we can't read may take it back.
        if p.clipped {
            return None;
        }
        let r = read(&p.text);
        let unnamed = |a: &Ask| i == 0 && unnamed_tasks(prompts, &r, matches!(a, Ask::Them)).contains(&id);
        match r.word_for(id, unnamed) {
            Some(true) => return Some(i),
            Some(false) => return None,
            None => {}
        }
    }
    None
}

/// Notes on terminal `sid` that its conversation made task `id`: the task an unnamed "queue it" can mean.
pub fn note_made(app: &App, sid: &str, id: i64) -> Result<()> {
    crate::board::session_event_with(app, sid, MADE, &format!("Added {}", rf("task", id)), None, Some(&id.to_string()))
}

/// A prompt as typed (`session_events.full`), or, for one stored before the board kept that, the line it
/// shows; and whether it was cut short on the way (the hook and the board each keep only so much).
fn typed(row: &Row) -> (String, bool) {
    let cut = |t: &str, at: usize| t.ends_with('…') && t.chars().count() >= at;
    match row.s("full") {
        Some(full) => (full.to_string(), cut(full, PROMPT_CUT_AT)),
        None => {
            let text = row.st("text");
            let clipped = cut(&text, 590);
            (text, clipped)
        }
    }
}

/// The prompt a human typed in terminal `sid`, since its Claude conversation began, that asks for task
/// `id` to start, if there's one and no later prompt takes it back.
pub fn owners_word(app: &App, sid: &str, id: i64) -> Result<Option<String>> {
    if sid.is_empty() {
        return Ok(None);
    }
    // Every prompt (the board's too, so a task made in reply to one isn't put on the prompt before it)
    // and every task the conversation made, oldest first.
    let rows = app.db.q(
        "SELECT id, kind, text, full, data FROM session_events WHERE session_id = ? AND kind IN ('prompt', ?)
           AND id > COALESCE((SELECT MAX(id) FROM session_events WHERE session_id = ? AND kind = 'start'), 0)
         ORDER BY id",
        p![sid, MADE, sid],
    )?;
    let mut prompts: Vec<Prompt> = vec![];
    for r in &rows {
        if r.st("kind") == MADE {
            if let (Some(p), Ok(made)) = (prompts.last_mut(), r.st("data").parse()) {
                p.made.push(made);
            }
            continue;
        }
        let (text, clipped) = typed(r);
        prompts.push(Prompt { text, made: vec![], clipped, board: r.s("data") == Some(BOARD_PROMPT) });
    }
    prompts.reverse();
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

    fn prompts(texts: &[&str], made: Option<i64>) -> Vec<Prompt> {
        texts.iter().enumerate().map(|(i, t)| Prompt { made: if i == 0 { made.into_iter().collect() } else { vec![] }, ..Prompt::new(t) }).collect()
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

    #[test]
    fn a_take_back_after_the_ask_in_the_same_prompt_takes_it_back() {
        for no in [
            "start T8. jk",
            "start T8 jk",
            "start T8. j/k",
            "start T8. Just kidding.",
            "start T8. On second thought, don't.",
            "start T8. on second thought no",
            "start T8. no.",
            "start T8. No",
            "start T8. nope",
            "start T8. Nah.",
            "start T8, scratch that",
            "start T8. Scratch that.",
            "start T8. Not now though.",
            "start T8. Not yet.",
            "start T8. Forget I said that.",
            "start T8. I changed my mind.",
            "start T8. Never mind that.",
            "start T8. Actually, don't.",
            "start T8. Please don't.",
            "start T8. Ignore that.",
            "start T8. Let's not.",
            "start T8. I take that back.",
            "start T8. Stop.",
            "queue it. jk",
            "start T8 and T9. no.",
        ] {
            assert!(!says_start(no), "{no}");
            assert!(read(no).holds(8) || no.starts_with("queue"), "{no}");
            assert_eq!(word_in(&prompts(&[no], None), 8), None, "{no}");
        }
    }

    #[test]
    fn a_take_back_before_an_ask_in_the_same_prompt_is_asked_again() {
        for yes in ["never mind. Start T8.", "scratch that, start T8", "nope. start T8", "Forget I said that. Start T8."] {
            assert_eq!(word_in(&prompts(&[yes], None), 8), Some(0), "{yes}");
        }
        // Words around an ask that only sound like a no.
        for yes in [
            "start T8 and stop the dev server",
            "start T8. Don't touch the CSS.",
            "no problem. start T8",
            "start T8, no worries",
            "start T8 and monitor the logs",
            "start T8 and decide on the copy as you go",
            "start T8. The fridge is empty",
        ] {
            assert_eq!(word_in(&prompts(&[yes], None), 8), Some(0), "{yes}");
        }
    }

    #[test]
    fn a_later_prompt_that_takes_back_every_start_takes_back_the_ask() {
        for back in [
            "no, don't",
            "No, don't.",
            "nope",
            "no",
            "nah",
            "never mind that",
            "never mind",
            "forget I said that",
            "forget it",
            "I changed my mind",
            "changed my mind",
            "jk",
            "on second thought, don't",
            "not now",
            "scratch that",
            "don't",
            "please don't",
            "let's not",
            "cancel that",
            "hold off",
        ] {
            assert_eq!(word_in(&prompts(&[back, "start T8"], None), 8), None, "{back}");
            assert_eq!(word_in(&prompts(&["start T8", back, "start T8"], None), 8), Some(0), "asked again after {back}");
        }
        // A later prompt that takes back nothing leaves the ask.
        for fine in ["thanks", "no problem", "and tell me when it's done", "what's the plan for T9?"] {
            assert_eq!(word_in(&prompts(&[fine, "start T8"], None), 8), Some(1), "{fine}");
        }
    }

    #[test]
    fn an_ask_instead_replaces_the_earlier_asks() {
        assert_eq!(word_in(&prompts(&["start T9 instead", "start T8"], None), 8), None);
        assert_eq!(word_in(&prompts(&["start T9 instead", "start T8"], None), 9), Some(0));
        assert_eq!(word_in(&prompts(&["actually, queue T9 instead", "start T8"], None), 8), None);
        assert_eq!(word_in(&prompts(&["start T8. Actually, start T9 instead."], None), 8), None);
        assert_eq!(word_in(&prompts(&["start T8. Actually, start T9 instead."], None), 9), Some(0));
        assert_eq!(word_in(&prompts(&["instead of T8, start T9"], None), 9), Some(0));
        assert_eq!(word_in(&prompts(&["instead of T8, start T9"], None), 8), None);
        assert_eq!(word_in(&prompts(&["start T9 rather than T8", "start T8"], None), 8), None);
        assert_eq!(word_in(&prompts(&["start T9 instead?", "start T8"], None), 8), None, "asking about another start puts this one in doubt");
    }

    #[test]
    fn a_deferral_after_a_comma_or_in_the_next_sentence_puts_it_off() {
        for no in [
            "start T8, first thing",
            "start T8, at six",
            "start T8 at six",
            "start T8 around ten",
            "start T8 by noon",
            "start T8, the moment T7 lands",
            "start T8 the minute T7 is merged",
            "start T8. Not now though.",
            "start T8, after lunch",
            "start T8. Sometime this week.",
            "start T8 at some point",
            "start T8 on my go",
            "start T8 by the time I'm back",
            "start T8 shortly",
            "start T8 at 6ish",
            "start T8 at half past",
        ] {
            assert!(!says_start(no), "{no}");
            assert_eq!(word_in(&prompts(&[no], None), 8), None, "{no}");
        }
    }

    #[test]
    fn an_aside_does_not_take_a_no_off_the_start_after_it() {
        for no in [
            "don't (like T9), start T8",
            "don't [T9], start T8",
            "don't, like T9, start T8",
            "do not - see T9 - start T8",
            "never (T9 aside) start T8",
        ] {
            assert!(!says_start(no) || !read(no).asks.contains(&Ask::Named(vec![8])), "{no}");
            assert_eq!(word_in(&prompts(&[no], None), 8), None, "{no}");
        }
        // A no on a task the sentence only names, with no start after it, stays on that task.
        assert_eq!(asks("start T8, not T9"), vec![Ask::Named(vec![8])]);
        assert_eq!(asks("start T8 (but not now)"), vec![]);
    }

    #[test]
    fn a_paste_after_a_line_ending_in_a_colon_is_no_ask() {
        for no in [
            "paste:\nstart T8",
            "CI failed with:\n\n    start T8",
            "my notes:\n\nstart T8",
            "my notes:\n\n\nstart T8\nstart T8",
            "the output:\r\nstart T8",
            "see below:\n\n> start T8",
            "fyi\n\n    start T8",
        ] {
            assert!(!says_start(no), "{no:?}");
        }
        assert!(says_start("my notes:\n\nfoo bar\n\nok, start T8"));
    }

    #[test]
    fn a_clipped_prompt_is_no_word() {
        let p = |text: &str, clipped| Prompt { clipped, ..Prompt::new(text) };
        assert_eq!(word_in(&[p("start T8", true)], 8), None);
        assert_eq!(word_in(&[p("thanks", true), p("start T8", false)], 8), None, "its unread end may take the start back");
        assert_eq!(word_in(&[p("start T8", false)], 8), Some(0));
    }

    /// A conversation, newest first: each prompt with the tasks made in reply to it.
    fn convo(said: &[(&str, &[i64])]) -> Vec<Prompt> {
        said.iter().map(|(t, made)| Prompt { made: made.to_vec(), ..Prompt::new(t) }).collect()
    }

    #[test]
    fn an_unnamed_ask_for_more_than_one_task_is_plural() {
        for plural in [
            "make tasks for A and B and queue them",
            "queue both",
            "start both of them",
            "kick them off",
            "queue them all",
            "start all of them",
            "start all the new tasks",
            "start the two new tasks",
            "queue these",
            "start those",
            "queue the new ones",
        ] {
            assert_eq!(asks(plural), vec![Ask::Them], "{plural}");
        }
        for one in ["queue it", "start the new task", "start this one", "kick it off", "queue that"] {
            assert_eq!(asks(one), vec![Ask::Unnamed], "{one}");
        }
        assert_eq!(read("don't queue them").held, vec![Ask::Them]);
    }

    #[test]
    fn a_plural_ask_covers_every_task_made_in_reply_to_it() {
        let c = convo(&[("make tasks for A and B and queue them", &[9, 10])]);
        assert_eq!(word_in(&c, 9), Some(0));
        assert_eq!(word_in(&c, 10), Some(0));
        assert_eq!(word_in(&c, 11), None, "not made in reply to it");
        let c = convo(&[("make three tasks for the footer, then start all of them", &[4, 5, 6])]);
        assert!([4, 5, 6].iter().all(|id| word_in(&c, *id) == Some(0)));
        // "it" is still the first one only.
        let c = convo(&[("make two tasks and queue it", &[9, 10])]);
        assert_eq!(word_in(&c, 9), Some(0));
        assert_eq!(word_in(&c, 10), None);
        // Not once a later prompt comes in, and not when it's held.
        assert_eq!(word_in(&convo(&[("thanks", &[]), ("make tasks for A and B and queue them", &[9, 10])]), 10), None);
        assert_eq!(word_in(&convo(&[("make tasks for A and B and queue them tomorrow", &[9, 10])]), 9), None);
        assert_eq!(word_in(&convo(&[("make tasks for A and B and queue them. jk", &[9, 10])]), 9), None);
        assert_eq!(word_in(&convo(&[("make tasks for A and B but don't queue them", &[9, 10])]), 9), None);
        assert_eq!(word_in(&convo(&[("don't make tasks, just queue them", &[9, 10])]), 9), None, "it asks for no new task");
    }

    #[test]
    fn queue_it_covers_the_task_made_just_before_it() {
        for ask in ["queue it", "start it", "ok, queue it", "great, kick it off", "yes, start it please", "Sounds good. Queue it.", "lgtm, queue it now"] {
            assert_eq!(word_in(&convo(&[(ask, &[]), ("make a task for the footer", &[9])]), 9), Some(0), "{ask}");
        }
        assert_eq!(word_in(&convo(&[("queue it", &[]), ("make a task for the footer but don't start it", &[9])]), 9), Some(0));
        assert_eq!(word_in(&convo(&[("queue it", &[10]), ("make a task for the footer", &[9])]), 9), Some(0), "it's still the one before");
        assert_eq!(word_in(&convo(&[("queue it", &[10]), ("make a task for the footer", &[9])]), 10), None);
        // Plural, for every task made in reply to the prompt before.
        let c = convo(&[("queue them", &[]), ("make tasks for the footer and the header", &[9, 10])]);
        assert_eq!((word_in(&c, 9), word_in(&c, 10)), (Some(0), Some(0)));
    }

    #[test]
    fn queue_it_is_no_word_when_it_may_mean_something_else() {
        let footer: &[i64] = &[9];
        for (ask, before, made) in [
            // Another prompt in between.
            ("queue it", "thanks", &[][..]),
            // The prompt before didn't ask for a task: the agent made it on its own.
            ("queue it", "fix the footer", footer),
            // Two tasks: which is "it"?
            ("queue it", "make tasks for the footer and the header", &[9, 10][..]),
            // Nothing made.
            ("queue it", "make a task for the footer", &[][..]),
            // The prompt says more than the ask: "it" may be what it speaks of.
            ("the dev server won't come up; start it", "make a task for the footer", footer),
            ("start it and check the logs", "make a task for the footer", footer),
            ("the build is red. queue it", "make a task for the footer", footer),
            // Asked about, put off or taken back.
            ("queue it?", "make a task for the footer", footer),
            ("queue it tomorrow", "make a task for the footer", footer),
            ("don't queue it", "make a task for the footer", footer),
            ("queue it. jk", "make a task for the footer", footer),
        ] {
            let c = convo(&[(ask, &[]), (before, made)]);
            assert!([9, 10].iter().all(|id| word_in(&c, *id).is_none()), "{ask} after {before}");
        }
        // The prompt before was the board's.
        let c = vec![Prompt::new("queue it"), Prompt { made: vec![9], board: true, ..Prompt::new("Plan the goal: make a task for the footer") }];
        assert_eq!(word_in(&c, 9), None);
        // The prompt before was clipped.
        let c = vec![Prompt::new("queue it"), Prompt { made: vec![9], clipped: true, ..Prompt::new("make a task for the footer") }];
        assert_eq!(word_in(&c, 9), None);
        // An older prompt's "queue it" counts only while it's the latest.
        let c = convo(&[("thanks", &[]), ("queue it", &[]), ("make a task for the footer", &[9])]);
        assert_eq!(word_in(&c, 9), None);
    }

    #[test]
    fn an_unnamed_ask_for_a_task_the_prompt_itself_makes_no_new_task_for_is_refused() {
        // "the dev server won't come up; start it" and then the agent makes a task: "it" may be the dev
        // server, so it's no word for the task.
        assert_eq!(word_in(&convo(&[("the dev server won't come up; start it", &[9])]), 9), None);
        assert_eq!(word_in(&convo(&[("the dev server won't come up. Start it.", &[9])]), 9), None);
        assert_eq!(word_in(&convo(&[("start it", &[9])]), 9), None);
    }
}
