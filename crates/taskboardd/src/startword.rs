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
//!
//! A no, a time or a condition holds only the start it's about: one in the start's own clause ("don't
//! start T8", "start T8 tomorrow"), a bare one just before it ("never, ever, start T8", "after lunch,
//! start T8", "once T7 lands, start T8"), or one after it that speaks of it ("start T8. jk", "start
//! T8, the moment T7 lands", "start T8. Do it after the deploy."). One about something else ("start T8
//! and tell me when it's done", "start T8, no rush", "start T8 but don't merge it") leaves it alone.
//! The latest prompt that speaks of a task's start decides: "don't start T8" after "start T8" takes it
//! back.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::app::App;
use crate::p;
use crate::util::*;

/// `session_events.data` on a prompt the board sent (`[task-board:T4] …`), which is no one's word.
pub const BOARD_PROMPT: &str = "board";

/// What stands for the middle of a prompt too long to keep whole ([`keep_ends`]).
pub const CUT_MARK: &str = "[… cut …]";

/// The hook before beta.16 kept a prompt's first 7999 characters and "…": what it dropped may take a
/// start back.
const OLD_HOOK_KEEP: usize = 8000;

/// `session_events.kind` for a task the terminal's conversation made (`tb task new`); `data` is its id.
pub const MADE: &str = "added";

/// Any board marker (`[task-board:T4]`, `[task-board:G2]`, `[task-board:J7]`, …): the prompt is the board's.
pub static MARKER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[task-board:[^\]\n]*\]").unwrap());

/// Where an ask can begin: the start of a sentence or of a clause.
const EDGE: &str = r"(?:^|[:,(\[—–]|\s-+\s|\b(?:and|then|but|so|also)\b)";
/// What may come before the start word in an ask ("ok,", "please", "go ahead and", "can you").
const LEAD: &str = r"(?:(?:ok(?:ay)?|alright|all\s+right|yes|yeah|yep|sure|great|cool|perfect|thanks|please|pls|now|then|so|also|just|go|go\s+ahead(?:\s+and)?|you\s+can|you\s+may|can\s+you|could\s+you|would\s+you|will\s+you|i\s+want\s+you\s+to|i[’']?d\s+like\s+you\s+to|let[’']?s|let\s+us|feel\s+free\s+to|time\s+to)\s*,?\s+)*";
/// The tasks an ask names: "T4", "task T4", "T4, T5 and T6".
const NAMED: &str = r"(?:task\s+)?[Tt]\d+(?:\s*,?\s*(?:and\s+|&\s*)?(?:task\s+)?[Tt]\d+)*";
/// What may come between the start word and the tasks it names: "both", "them:".
const LISTED: &str = r"(?:(?:them|these|those|both|all\s+of\s+them)\s*[:—–-]\s*|both\s+)?";
/// The task an ask doesn't name: "it", "this", "the new task", "them", "both of them", "the first one".
const UNNAMED: &str = concat!(
    r"(?:(?:both|all|each)\s+of\s+(?:them|these|those|the\s+(?:new\s+)?(?:tasks|ones))|them\s+(?:all|both)|",
    r"all\s+(?:the\s+|these\s+|those\s+)?(?:new\s+)?(?:tasks|ones)|all\s+(?:two|three|four|five|six)|",
    r"(?:the|these|those|my|your|our)\s+(?:two|three|four|five|six)\s+(?:new\s+)?(?:tasks|ones)|",
    r"the\s+first\s+(?:new\s+)?(?:one|task)|",
    r"(?:the|this|that|these|those|my|your|our)\s+(?:new\s+)?(?:one|ones|task|tasks)|it|this|that|them|these|those|both)"
);
/// An unnamed ask that means more than one task ("them", "both", "these", "the new tasks").
static PLURAL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:them|these|those|both|all|ones|tasks)\b").unwrap());
/// An unnamed ask for the first of the tasks made ("queue the first one").
static FIRST_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^the\s+first\b").unwrap());
/// An unnamed ask that can mean a task the prompt just spoke of ("T8 is ready, start it").
static PRONOUN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:it|this|that|(?:this|that)\s+(?:one|task))$").unwrap());
/// A clause that speaks of one task as ready or done ("T8 is ready", "T8's good", "T8 looks fine").
static TASK_SUBJECT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[.!?;\n,:(—–]\s*|\b(?:and|so|but|ok(?:ay)?)\s+)(?:task\s+)?[Tt](\d+)\s*(?:is|[’']s|looks|seems|sounds|has\s+been)\b").unwrap()
});
/// A prompt that's nothing but an unnamed ask ("queue it", "ok, start them", "great, kick it off"): its
/// "it" can only be what the conversation just made. Asking to hear back ("queue it and let me know when
/// it's done") says nothing more about "it".
static BARE_ASK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)^(?:(?:ok(?:ay)?|alright|all\s+right|yes|yeah|yep|yup|sure|great|cool|nice|perfect|awesome|good|thanks|thank\s+you|ty|please|pls|looks\s+good|lgtm|sounds\s+good|go\s+ahead(?:\s+and)?|now|then|so|and|just|you\s+can|can\s+you|could\s+you|let[’']?s|that[’']?s\s+(?:fine|great|good|perfect))[\s,.!]*)*(?:{VERB}\s+(?:{UNNAMED})|kick\s+(?:{UNNAMED})\s+off)(?:\s+(?:up|off|now|please|pls|too|for\s+me|right\s+away|right\s+now|asap))*(?:[\s,]*(?:and\s+)?(?:let\s+me\s+know|tell\s+me|ping\s+me|lmk)\b[^.!?;\n]*)?[\s.!]*(?:(?:thanks|thank\s+you|ty|please|pls)[\s.!]*)?$"
    ))
    .unwrap()
});
/// What may follow the task in an ask: the end of the clause, or a word that goes on with it ("start T8
/// without the migration", "start T8 on Monday"). Anything else ("start T8 failed", "start T8 is
/// broken") makes it no ask. A time, a condition or a question is read with the clause and the sentence.
const TAIL: &str = concat!(
    r"(?:\s*$|\s*[:,()\[\]—–]|\s+-|\s+(?:and|then|now|please|pls|too|again|up|off|asap|instead|for|right\s+away|right\s+now|",
    r"without|with|but|so|also|or|not|first|next|immediately|today|tomorrow|tonight|later|on|in|at|by|after|before|when|whenever|",
    r"once|if|unless|until|till|as|this|down|around|thanks|thank|ty|it|it[’']s|its|from|using|via|to|while|since|because|anyway|jk)\b)"
);
/// The start words.
const VERB: &str = r"(?:start|queue|begin|launch|kick\s+off)(?:\s+up)?(?:\s+(?:work(?:ing)?\s+)?on)?";
/// The start words that ask only with a task named ("run T8", "pick up T8", but not "run it").
const NAMED_VERB: &str = r"(?:run|pick\s+up)";

/// The goals an ask names: "G2", "goal G2", "the goal G2", "G2 and G3".
const NAMED_GOAL: &str = r"(?:the\s+)?(?:goal\s+)?[Gg]\d+(?:\s*,?\s*(?:and\s+|&\s*)?(?:goal\s+)?[Gg]\d+)*";
/// The goal an ask doesn't name: "the goal", "this goal", "the whole goal".
const UNNAMED_GOAL: &str = r"(?:(?:the|this|that|my|your|our|its)\s+(?:whole\s+|new\s+)?goal)";
/// The start words for a goal, which also runs.
const GOAL_VERB: &str = r"(?:start|queue|begin|launch|kick\s+off|run)(?:\s+up)?(?:\s+(?:work(?:ing)?\s+)?on)?";

/// What a start is read on: tasks (`tb start T4`) or goals (`tb start G2`).
pub struct Kind {
    /// A start word at the start of a clause, on one, with nothing after it that makes it a story.
    ask: Regex,
    /// A start word on one anywhere: an ask, or (when it's no ask) a mention that takes the start back.
    mention: Regex,
    /// One named, with its number.
    ids: Regex,
}

fn kind(named: &str, unnamed: &str, verb: &str, named_verb: Option<&str>, ids: &str) -> Kind {
    let mut on = format!(
        r"(?:\b{verb}\s+(?:{LISTED}(?P<named>{named})|(?P<unnamed>{unnamed}))|\bkick\s+(?:(?P<named2>{named})|(?P<unnamed2>{unnamed}))\s+off"
    );
    if let Some(v) = named_verb {
        on.push_str(&format!(r"|\b{v}\s+{LISTED}(?P<named3>{named})"));
    }
    on.push(')');
    Kind { ask: Regex::new(&format!(r"(?i){EDGE}\s*{LEAD}{on}{TAIL}")).unwrap(), mention: Regex::new(&format!(r"(?i){on}\b")).unwrap(), ids: Regex::new(ids).unwrap() }
}

/// Starts of tasks.
pub static TASKS: Lazy<Kind> = Lazy::new(|| kind(NAMED, UNNAMED, VERB, Some(NAMED_VERB), r"\b[Tt](\d+)\b"));
/// Runs of goals.
pub static GOALS: Lazy<Kind> = Lazy::new(|| kind(NAMED_GOAL, UNNAMED_GOAL, GOAL_VERB, None, r"\b[Gg](\d+)\b"));
/// A sentence and what ends it.
static SENTENCE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?P<s>[^.!?;\n…]*)(?P<end>[.!?;\n…]*)").unwrap());
/// Dotted words whose dots end no sentence ("6 a.m.", "e.g.").
static DOTTED_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b([ap])\.m\.|\b(e)\.g\.|\b(i)\.e\.").unwrap());
/// Where a clause of a sentence ends and the next begins: a comma, a colon, a bracket or a dash, or a
/// joining word ("and", "then", "but") that doesn't join two tasks ("T8 and T9").
static CLAUSE_EDGE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)[,:()\[\]{}—–]|\s-+\s|\b(?:and|then|but|so|or|also|plus|while)\b").unwrap());
/// What follows a joining word that joins two tasks, not two clauses.
static JOINS_TASKS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^\s*(?:task\s+|goal\s+)?[TtGg]\d+\b").unwrap());
/// A word that says no ("don't", "never", "no need to").
static NEG_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:not|never|no|nobody|nothing|none|nor|neither|without|cannot|dont|wont|cant|shouldnt|mustnt|avoid|refrain|forbid|prohibit|instead\s+of|rather\s+than|except|hold\s+off|forget|skip|cancel|nope|nah)\b|n[’']t\b").unwrap()
});
/// "No" that doesn't say no to anything.
static NOT_NEG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bno\s+(?:problem|worries|prob)\b").unwrap());
/// A clause that's only a no, which falls on the next start in the sentence ("never, ever, start T8",
/// "do not, under any circumstances, start T8", "I forbid you: start T8", "don't (like T9), start T8").
/// A bare "no" or "nope" isn't one: "no, start T8" answers the agent.
static NO_CLAUSE_RE: Lazy<Regex> = Lazy::new(|| {
    let fill = r"(?:please|pls|ok(?:ay)?|i|you|we|just|really|seriously|ever|again|now|yet|repeat|but|and|so|actually|though|tho|at\s+all|_|under\s+any\s+circumstances|whatever\s+you\s+do|for\s+any\s+reason|no\s+matter\s+what|(?:task\s+)?[TtGg]\d+)";
    let no = r"(?:not|never|don[’']?t|do\s+not|cannot|can[’']?t|can\s+not|mustn[’']?t|must\s+not|won[’']?t|will\s+not|shouldn[’']?t|should\s+not|forbid|prohibit|refrain|nobody|no\s+one|under\s+no\s+circumstances)";
    Regex::new(&format!(r"(?i)^\s*(?:{fill}\s+)*{no}(?:\s+{fill})*\s*[!.]*\s*$")).unwrap()
});
/// A condition: the start is for when something else happens.
static CONDITION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:when|whenever|once|if|unless|after|until|till|til|before|provided|providing|assuming|soon|long\s+as|in\s+case|depending|wait|waiting|",
        r"the\s+(?:moment|minute|second|instant|day|time)|by\s+the\s+time|on\s+(?:my|your)\s+(?:go|signal|mark|word|say))\b"
    ))
    .unwrap()
});
/// A time: the start isn't for now. Whole words only: "never mind" has no "min" in it, and "monitor"
/// no "mon".
static TIME_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:until|till|later|tomorrow|tmrw|tmr|tonight|today|overnight|weekends?|mornings?|afternoons?|evenings?|nights|midnight|noon|",
        r"(?P<day>monday|tuesday|wednesday|thursday|friday|saturday|sunday|mon|tues?|wed|thu|thurs?|fri|sat|",
        r"january|february|march|april|june|july|august|september|october|november|december|jan|feb|mar|apr|jun|jul|aug|sept?|oct|nov|dec)|",
        r"next\s+(?:week|month|time|sprint|year)|this\s+(?:week|weekend|month|evening|afternoon|morning)|end\s+of\s+(?:the\s+)?(?:day|week)|eod|eow|",
        r"wait|waiting|hold\s+(?:off|on)|whenever|eventually|soon|afterwards?|yet|schedule[ds]?|o[’']?clock|hours?|minutes?|mins?|days?|weeks?|months?|",
        r"first\s+thing|lunch(?:time)?|dinner|bedtime|sometime|someday|shortly|momentarily|at\s+some\s+point|half\s+past|quarter\s+(?:past|to)|",
        r"down\s+the\s+(?:road|line)|at\s+(?:your|my)\s+(?:earliest\s+)?convenience|when(?:ever)?\s+convenient|\d{1,2}(?:st|nd|rd|th)|",
        r"(?:one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve)\s*(?:am|pm)|",
        r"(?:at|by|around|about|past)\s+(?:one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|dawn|dusk|night)|\d{1,2}\s*ish|",
        r"in\s+(?:a|an|one|two|three|four|five|few|couple|\d+)|at\s+\d+|\d{1,2}\s*[ap]\.?m|\d{1,2}:\d{2}|\d{1,2}/\d{1,2})\b"
    ))
    .unwrap()
});
/// A word before a day or a month that makes it part of a name, not a time ("the Monday report").
static NAME_BEFORE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:the|a|an|my|your|our|their|his|her|its|last)\s+$").unwrap());
/// A word after it, which a name has ("the Monday report")…
static NAME_AFTER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^\s+[a-z]").unwrap());
/// …unless it says when ("the Monday after next", "the Monday morning").
static TIME_AFTER_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s+(?:after|before|following|morning|afternoon|evening|night|at|by|then)\b").unwrap());
/// A time that's said not to be the time ("start T8 now, not later"), and "as soon as possible".
static NOT_A_TIME_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bnot\s+(?:later|tomorrow|tonight|afterwards?|next\s+\w+)\b|\bas\s+soon\s+as\s+possible\b").unwrap());
/// What may come before the time or condition in a clause that's only that ("on the 15th", "only
/// after T7 lands", "then"), so it falls on a start next to it.
static ONLY_LEAD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s_]*(?:(?:and|but|so|or|also|then|just|actually|ok(?:ay)?|maybe|probably|ideally|preferably|say|i\s+mean|um+|uh+|on|at|by|in|for|around|about|from|the|some|only|even|please|no\s+earlier\s+than|not\s+(?:before|until|till))\s+)*$").unwrap()
});
/// Someone doing something, after a time: the clause is about that, not about the start ("tomorrow
/// I'll check the footer").
static SUBJECT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:i|i[’']ll|i[’']m|i[’']d|we|we[’']ll|you[’']ll|they|he|she|it[’']s|that[’']s|there)\b").unwrap());
/// "I mean", which says nothing of anyone doing anything ("tomorrow, I mean").
static I_MEAN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bi\s+mean\b").unwrap());
/// A clause that only goes on to the next thing ("then wait for review", "and wait"): no word on the
/// start before it.
static NEXT_STEP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^\s*(?:and\s+)?then\b\s*\S|^\s*and\b").unwrap());
/// A clause that's only "then", at the end ("start T8 then"): at that time, later.
static THEN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^\s*then\s*$").unwrap());
/// A clause that says nothing ("thanks", "please", "ok").
static FILLER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s_!]*(?:(?:thanks|thank\s+you|ty|please|pls|ok(?:ay)?|though|tho|sure|right|yes|i\s+mean)[\s!]*)*$").unwrap()
});
/// A clause that puts off what came before it by pointing at it ("do it after the deploy", "do that
/// tomorrow", "it's for next week").
static DO_IT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)^\s*(?:(?:ok(?:ay)?|but|so|just|please|actually|and)\s+)*(?:(?:do|run|handle|tackle)\s+(?:it|that|this|them|those|these|both)\b|",
        r"(?:it|that|this|they|those|these)(?:[’']s|[’']re|\s+is|\s+are)\s+(?:meant\s+|planned\s+)?(?:for|due)\b)"
    ))
    .unwrap()
});
/// Words that take back what came before them ("wait", "never mind", "jk", "on second thought, don't"),
/// in the prompt or in an earlier one, when no task comes after them in the sentence.
static HALT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:wait|hold\s+(?:off|on|up)|never\s*mind|nvm|scratch\s+that|cancel\s+(?:that|it|this|them|these|those)|",
        r"forget\s+(?:it|that|this|them|about\s+(?:it|that|this|them)|(?:what\s+)?i\s+said)|not\s+(?:yet|now|today|right\s+now|so\s+fast)|",
        r"don[’']?t\s+(?:do\s+(?:it|that|this)|bother\s*$|start|queue|begin|launch|kick)|do\s+not\s+(?:do\s+(?:it|that|this)|start|queue|begin|launch|kick)|",
        r"actually\s+no|jk|j/k|just\s+(?:kidding|joking)|kidding|joking|psych|on\s+second\s+thoughts?|second\s+thoughts?|",
        r"change[ds]?\s+(?:of\s+)?(?:my\s+)?mind|changing\s+my\s+mind|change\s+of\s+plans?|take\s+(?:it|that|this)\s+back|belay\s+that|",
        r"ignore\s+(?:that|this|it|what\s+i\s+said|my\s+last)|disregard|undo\s+(?:that|it|this)|let[’']?s\s+not|rather\s+not|better\s+not|or\s+not|no\s+longer)\b"
    ))
    .unwrap()
});
/// "Wait" or "hold on" as the next step ("then wait for review") or said not to ("no need to wait",
/// "don't wait for me"), not a take-back.
static STEP_BEFORE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:then|and|no\s+need\s+to|(?:don[’']?t|do\s+not|doesn[’']?t)(?:\s+need\s+to)?|never|without)\s*$").unwrap());
/// A sentence that ends on a no ("on second thought, don't", "please don't", "I'd rather you not").
static ENDS_NO_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:don[’']?t|do\s+not|not|never)\s*(?:please|pls|yet|now|though|tho|after\s+all|anymore|any\s+more)?\s*[,!]*\s*$").unwrap()
});
/// A sentence that starts with a bare no ("no.", "nope", "no, don't", "nah"), not one that starts with a
/// no on something ("no need to ask", "no rush").
static STARTS_NO_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s,]*(?:(?:oh|ah|um+|uh+|hm+|actually|wait|ok(?:ay)?|well|so|and|but)\b[\s,]*)*(?P<no>(?:no+|nope|nah|naw|nay|negative)(?:[\s,]+(?:no+|nope|nah))*)\s*(?:[,!]|$)").unwrap()
});
/// Words that stop things, which take back what came before only in a sentence that names no task
/// ("stop", but not "start T8 and stop the dev server").
static STOP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:stop|pause|halt|abort)\b").unwrap());
/// An ask that replaces the ones before it ("start T9 instead").
static INSTEAD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:instead|rather)\b").unwrap());
/// A word that holds a task off when a clause names it outside a start ("hold off on T8", "T8 can wait").
static HOLD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:not|never|no|don[’']?t|do\s+not|wait|hold|pause|stop|later|leave|alone|yet|skip|cancel|forget|instead)\b|n[’']t\b").unwrap()
});
/// The prompt asks for a new task, the one an unnamed ask ("make a task and queue it") can mean.
static MAKE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:make|create|add|file|open|write|set\s+up|log|draft|a\s+new)\b[^.!?;\n]*\b(?:task|ticket|card|one)s?\b").unwrap()
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
/// A line that hands over what's on the lines right after it without a colon ("here's the output.").
static HANDS_OVER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:here[’']?s|here\s+(?:is|are)|below\s+is|this\s+is|these\s+are|i\s+got)\b.*\b(?:output|log|logs|trace|traceback|error|errors|result|results|diff|stdout|stderr|dump|transcript|thread|response|console)\s*[.!]*\s*$").unwrap()
});
/// A line that reads as pasted (a log line, a quote, a prompt, a diff, a list).
static PASTED_LINE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:\s|[>$#|\[{<+*\-•]|\d{1,4}[:\-/.)\]])").unwrap());

/// One ask for a start in a prompt: for the tasks it names, for a task it doesn't name ("queue it"), for
/// tasks it doesn't name ("queue them"), or for the first of the tasks made ("queue the first one").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Named(Vec<i64>),
    Unnamed,
    Them,
    First,
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

/// A prompt for the board, at most `n` characters: whole when it fits, or its start and its end with
/// [`CUT_MARK`] between, so the check reads both the ask at the start and a take-back at the end. The
/// cut falls on line breaks when there are some near it.
pub fn keep_ends(text: &str, n: usize) -> String {
    let c: Vec<char> = text.chars().collect();
    if c.len() <= n {
        return text.to_string();
    }
    let room = n.saturating_sub(CUT_MARK.chars().count() + 2);
    let (head, tail) = (room * 3 / 5, room - room * 3 / 5);
    let near = room / 10;
    let h = (head.saturating_sub(near)..head).rev().find(|&i| c[i] == '\n');
    let from = c.len() - tail;
    let t = (from..(from + near).min(c.len())).find(|&i| c[i] == '\n');
    let a: String = c[..h.unwrap_or(head)].iter().collect();
    let b: String = c[t.map(|i| i + 1).unwrap_or(from)..].iter().collect();
    let edge = |line: Option<usize>| if line.is_some() { "\n" } else { " " };
    format!("{a}{}{CUT_MARK}{}{b}", edge(h), edge(t))
}

/// The owner's own words in `text`: code, quotes, the rest of a line after "says:", the lines after a
/// line that ends in a colon (up to a blank line) or that hands over output ("here's the log."), and
/// lines that read as pasted all come out.
pub fn owners_text(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    // The middle of a long prompt, cut on its way in: no words, but a paste goes on through it.
    let text = text.replace(CUT_MARK, "…");
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
            lead = pasted;
        } else if line.trim_end().ends_with(':') {
            (pasted, lead) = (true, true);
        } else if HANDS_OVER_RE.is_match(line) {
            // Without a colon the paste must start on the next line: "here's the log.\n\nstart T8" asks.
            (pasted, lead) = (true, false);
        }
        out.push(own.to_string());
    }
    out.join("\n")
}

/// One start word in a sentence: where its verb is, the task words it's on, and what it asks for.
struct Mention {
    verb: usize,
    span: (usize, usize),
    ask: Ask,
    /// A start word that asks only in an ask's shape ("run T8", not "run T8's tests"): out of that shape
    /// it says nothing on the start.
    soft: bool,
}

/// The task span and the ask of one mention.
fn mention_of(k: &Kind, c: &regex::Captures<'_>) -> Option<Mention> {
    let verb = c.get(0)?.start();
    let soft = c.name("named3");
    if let Some(n) = c.name("named").or(c.name("named2")).or(soft) {
        return Some(Mention { verb, span: (n.start(), n.end()), ask: Ask::Named(ids_in(k, n.as_str())), soft: soft.is_some() });
    }
    let u = c.name("unnamed").or(c.name("unnamed2"))?;
    let ask = if FIRST_RE.is_match(u.as_str()) {
        Ask::First
    } else if PLURAL_RE.is_match(u.as_str()) {
        Ask::Them
    } else {
        Ask::Unnamed
    };
    Some(Mention { verb, span: (u.start(), u.end()), ask, soft: false })
}

/// The words in `s` that say no, less the "no" of "no problem".
fn negs_in(s: &str) -> Vec<regex::Match<'_>> {
    let not: Vec<_> = NOT_NEG_RE.find_iter(s).collect();
    NEG_RE.find_iter(s).filter(|n| !not.iter().any(|x| x.start() <= n.start() && n.end() <= x.end())).collect()
}

/// The times and conditions in `s`, less a day or month in a name ("the Monday report", but not "the
/// Monday after next") and a time it says isn't the time ("not later").
fn defers_in(s: &str) -> Vec<regex::Match<'_>> {
    let not: Vec<_> = NOT_A_TIME_RE.find_iter(s).collect();
    let named = |m: &regex::Match| {
        let day = TIME_RE.captures_at(s, m.start()).and_then(|c| c.name("day")).is_some_and(|d| d.start() == m.start());
        day && NAME_BEFORE_RE.is_match(&s[..m.start()]) && NAME_AFTER_RE.is_match(&s[m.end()..]) && !TIME_AFTER_RE.is_match(&s[m.end()..])
    };
    let mut out: Vec<_> = TIME_RE
        .find_iter(s)
        .chain(CONDITION_RE.find_iter(s))
        .filter(|m| !not.iter().any(|x| x.start() <= m.start() && m.end() <= x.end()) && !named(m))
        .collect();
    out.sort_by_key(|m| m.start());
    out
}

/// Whether clause `t` is only a time or a condition ("after lunch", "first thing", "on the 15th", "once
/// T7 lands"), which falls on a start next to it, and not a clause of its own that has one in it
/// ("tell me when it's done", "tomorrow I'll check the footer").
fn only_a_time(t: &str) -> bool {
    defers_in(t).iter().any(|m| {
        let condition = CONDITION_RE.find_at(t, m.start()).is_some_and(|c| c.start() == m.start());
        ONLY_LEAD_RE.is_match(&t[..m.start()]) && (condition || !SUBJECT_RE.is_match(&I_MEAN_RE.replace_all(&t[m.end()..], "")))
    })
}

/// The clauses of sentence `s`, as spans of it. A clause begun by a joining word keeps it ("then wait").
fn clauses(s: &str) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut at = 0;
    for m in CLAUSE_EDGE_RE.find_iter(s) {
        let word = m.as_str().starts_with(|c: char| c.is_alphabetic());
        if word && JOINS_TASKS_RE.is_match(&s[m.end()..]) {
            continue;
        }
        out.push((at, m.start()));
        at = if word { m.start() } else { m.end() };
    }
    out.push((at, s.len()));
    out
}

/// Whether sentence `s` asks for a new task (and doesn't say not to: "don't make a task, just queue it").
fn makes_in(s: &str) -> bool {
    MAKE_RE.find(s).is_some_and(|m| negs_in(&s[..m.start()]).is_empty())
}

/// Where in sentence `s` it takes back every start before it, if it does: a take-back word with no task
/// after it in the sentence ("start T8, scratch that", "jk", "no.", "on second thought, don't").
fn halt_in(k: &Kind, s: &str) -> Option<usize> {
    let mentioned = k.mention.captures_iter(s).filter_map(|c| mention_of(k, &c)).map(|m| m.span.0);
    let refs: Vec<usize> = k.ids.find_iter(s).map(|m| m.start()).chain(mentioned).collect();
    let last = |at: usize| !refs.iter().any(|r| *r >= at);
    let step = |m: &regex::Match| m.as_str().to_lowercase().starts_with(['w', 'h']) && STEP_BEFORE_RE.is_match(&s[..m.start()]);
    let mut at: Vec<usize> = HALT_RE.find_iter(s).filter(|m| last(m.end()) && !step(m)).map(|m| m.start()).collect();
    at.extend(ENDS_NO_RE.find(s).filter(|m| last(m.end())).map(|m| m.start()));
    if let Some(no) = STARTS_NO_RE.captures(s).and_then(|c| c.name("no")) {
        if last(no.end()) {
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
    read_of(&TASKS, text)
}

/// What `text` says about starting tasks or running goals.
pub fn read_of(k: &Kind, text: &str) -> Reading {
    let own = owners_text(text);
    let own = DOTTED_RE.replace_all(&own, |c: &regex::Captures| match (c.get(1), c.get(2), c.get(3)) {
        (Some(ap), _, _) => format!("{}m", ap.as_str()),
        (_, Some(e), _) => format!("{}g", e.as_str()),
        (_, _, Some(i)) => format!("{}e", i.as_str()),
        _ => String::new(),
    });
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
        let asked = c.name("end").is_some_and(|m| m.as_str().contains('?'));
        sentence(k, &own, base, s, asked, &mut steps);
        if let Some(at) = halt_in(k, s) {
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

/// The steps of sentence `s` (at `base` in `own`), clause by clause. A start is an ask when it has an
/// ask's shape, the sentence isn't a question, and nothing in its own clause, nor a bare no, time or
/// condition in a clause just before it, holds it. A clause that's only a time after the starts ("start
/// T8, the moment T7 lands") or puts off "it" ("do it tomorrow") holds every start before it.
fn sentence(k: &Kind, own: &str, base: usize, s: &str, asked: bool, steps: &mut Vec<(usize, Step)>) {
    let mut ask_spans = vec![];
    let mut at = 0;
    while let Some(m) = k.ask.captures_at(s, at) {
        let Some(m) = mention_of(k, &m) else { break };
        ask_spans.push(m.span);
        at = m.span.1;
    }
    let mentions: Vec<Mention> = k
        .mention
        .captures_iter(s)
        .filter_map(|c| mention_of(k, &c))
        .filter(|m| !m.soft || ask_spans.contains(&m.span) || !s[m.span.1..].starts_with(['\'', '’']))
        .collect();
    // "start T9 instead": the asks before this one are off.
    if !mentions.is_empty() && INSTEAD_RE.is_match(s) {
        steps.push((base, Step::Hold(Ask::Unnamed)));
    }
    let cl = clauses(s);
    let starts_after = |at: usize| mentions.iter().any(|m| m.verb >= at);
    // A bare no, time or condition that falls on the next start in the sentence.
    let mut pending = false;
    let mut i = 0;
    while i < cl.len() {
        let (from, mut to) = cl[i];
        let ms: Vec<&Mention> = mentions.iter().filter(|m| from <= m.verb && m.verb < to.max(from + 1)).collect();
        if ms.is_empty() {
            let t = &s[from..to];
            // A no, or a heading with a time in it ("tomorrow's plan: start T8"), before a start.
            let heading = s[to..].starts_with(':') && !defers_in(t).is_empty();
            if (NO_CLAUSE_RE.is_match(t) || heading) && starts_after(to) {
                pending = true;
            } else if only_a_time(t) && !THEN_RE.is_match(t) {
                if starts_after(to) {
                    pending = true;
                } else if cl[i + 1..].iter().all(|(a, b)| FILLER_RE.is_match(&s[*a..*b])) && !NEXT_STEP_RE.is_match(t) {
                    steps.push((base + from, Step::Hold(Ask::Unnamed)));
                }
            } else if THEN_RE.is_match(t) && i > 0 && cl[i + 1..].iter().all(|(a, b)| FILLER_RE.is_match(&s[*a..*b])) {
                steps.push((base + from, Step::Hold(Ask::Unnamed)));
            }
            if DO_IT_RE.is_match(t) && !defers_in(t).is_empty() {
                steps.push((base + from, Step::Hold(Ask::Unnamed)));
            }
            loose_holds(k, base, from, t, &[], steps);
            i += 1;
            continue;
        }
        // The clause runs on to the end of the last task its starts name ("start them: T8 and T9").
        let last = ms.iter().map(|m| m.span.1).max().unwrap_or(to);
        while i + 1 < cl.len() && cl[i + 1].0 < last {
            i += 1;
            to = cl[i].1;
        }
        let t = &s[from..to];
        let timed = !defers_in(t).is_empty();
        for m in mentions.iter().filter(|m| from <= m.verb && m.verb < to) {
            let shaped = ask_spans.contains(&m.span);
            if m.soft && !shaped && !asked && !timed && !pending {
                continue;
            }
            let clean = shaped && !asked && !pending && !timed && negs_in(&s[from..m.verb]).is_empty();
            let ask = if clean { resolved(k, own, base + m.verb, &m.ask, s, m.span) } else { m.ask.clone() };
            steps.push((base + m.span.0, if clean { Step::Ask(ask) } else { Step::Hold(ask) }));
        }
        let spans: Vec<(usize, usize)> = mentions.iter().filter(|m| m.span.0 >= from).map(|m| (m.span.0 - from, m.span.1 - from)).collect();
        loose_holds(k, base, from, t, &spans, steps);
        pending = false;
        i += 1;
    }
}

/// The task an unnamed "it" means when the prompt, just before it, spoke of one task as ready ("T8 is
/// ready, start it"); otherwise the ask as it is.
fn resolved(k: &Kind, own: &str, at: usize, ask: &Ask, s: &str, span: (usize, usize)) -> Ask {
    if *ask != Ask::Unnamed || !PRONOUN_RE.is_match(&s[span.0..span.1]) {
        return ask.clone();
    }
    let before = &own[..at];
    let Some(c) = TASK_SUBJECT_RE.captures_iter(before).last() else { return ask.clone() };
    let after = &before[c.get(1).map(|m| m.end()).unwrap_or(0)..];
    match (c[1].parse(), k.ids.is_match(after)) {
        (Ok(id), false) => Ask::Named(vec![id]),
        _ => ask.clone(),
    }
}

/// The tasks clause `t` (at `from` in its sentence) names outside its starts (`spans`), held when the
/// clause holds them off or puts them off ("hold off on T8", "T8 can wait", "not T9", "but T9 tomorrow").
fn loose_holds(k: &Kind, base: usize, from: usize, t: &str, spans: &[(usize, usize)], steps: &mut Vec<(usize, Step)>) {
    let loose: Vec<i64> = k
        .ids
        .captures_iter(t)
        .filter(|c| {
            let m = c.get(0).unwrap();
            !spans.iter().any(|(a, b)| *a <= m.start() && m.end() <= *b)
        })
        .filter_map(|c| c[1].parse().ok())
        .collect();
    if !loose.is_empty() && (HOLD_RE.is_match(t) || !defers_in(t).is_empty()) {
        steps.push((base + from + t.len(), Step::Hold(Ask::Named(loose))));
    }
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
    ids_in(&TASKS, text)
}

/// The tasks or goals `text` names (T4 → 4, G2 → 2).
fn ids_in(k: &Kind, text: &str) -> Vec<i64> {
    k.ids.captures_iter(text).filter_map(|c| c[1].parse().ok()).collect()
}

/// One prompt of the conversation, for [`word_in`].
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    pub text: String,
    /// The tasks the conversation made in reply to this prompt (after it, before the next one), in order.
    pub made: Vec<i64>,
    /// Only the start of the prompt reached the board (the hook before beta.16 dropped the rest): what
    /// it says past that is unknown. A prompt cut by [`keep_ends`] keeps its end, and isn't this.
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
/// one. "The first one" is the first of them either way. A prompt with more in it ("the dev server won't
/// come up; start it") may mean something else.
fn unnamed_tasks(prompts: &[Prompt], r: &Reading, ask: &Ask) -> Vec<i64> {
    let Some(p) = prompts.first() else { return vec![] };
    let first = |made: &[i64]| made.first().copied().into_iter().collect();
    if r.makes {
        return if *ask == Ask::Them { p.made.clone() } else { first(&p.made) };
    }
    let Some(prev) = prompts.get(1) else { return vec![] };
    if prev.boards() || prev.clipped || !read(&prev.text).makes || !BARE_ASK_RE.is_match(owners_text(&p.text).trim()) {
        return vec![];
    }
    match ask {
        Ask::Them => prev.made.clone(),
        Ask::First => first(&prev.made),
        _ if prev.made.len() == 1 => prev.made.clone(),
        _ => vec![],
    }
}

/// Which prompt, newest first, is the word to start task `id`. The latest prompt that speaks of the
/// task's start decides: if it takes the start back, puts it off or asks about it, there's no word. A
/// prompt that takes back every start without naming one ("nope", "never mind that", "I changed my
/// mind") or asks for another start in place of the earlier ones ("start T9 instead") speaks of them all.
/// An unnamed ask ("queue it", "queue them") is the word only in the latest prompt, for the tasks
/// [`unnamed_tasks`] says it means. A prompt whose end never reached the board ([`Prompt::clipped`]) is
/// no word, since its end may take the ask back, but it takes back only what it says.
pub fn word_in(prompts: &[Prompt], id: i64) -> Option<usize> {
    for (i, p) in prompts.iter().enumerate() {
        if p.boards() {
            continue;
        }
        let r = read(&p.text);
        let unnamed = |a: &Ask| i == 0 && unnamed_tasks(prompts, &r, a).contains(&id);
        match r.word_for(id, unnamed) {
            Some(true) if p.clipped => {}
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
/// shows; and whether only its start reached the board. Today the hook and the board keep a long
/// prompt's start and end ([`keep_ends`]); the hook before beta.16 kept its first 7999 characters and
/// "…", so a stored prompt of just that length ending in "…" is one of those, and so is a line that
/// filled the history's 600.
fn typed(row: &Row) -> (String, bool) {
    let cut = |t: &str, at: std::ops::RangeInclusive<usize>| t.ends_with('…') && at.contains(&t.chars().count());
    match row.s("full") {
        Some(full) => (full.to_string(), cut(full, OLD_HOOK_KEEP - 10..=OLD_HOOK_KEEP)),
        None => {
            let text = row.st("text");
            let clipped = cut(&text, 590..=600);
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

/// Which prompt, newest first, is the word to run goal `id`, each with whether "the goal" in it means
/// `id` (the conversation's goal then). Read like [`word_in`]: the latest prompt that speaks of the run
/// decides, and a prompt whose end never reached the board is no word.
pub fn goal_word_in(prompts: &[(Prompt, bool)], id: i64) -> Option<usize> {
    for (i, (p, ours)) in prompts.iter().enumerate() {
        if p.boards() {
            continue;
        }
        match read_of(&GOALS, &p.text).word_for(id, |_| *ours) {
            Some(true) if p.clipped => {}
            Some(true) => return Some(i),
            Some(false) => return None,
            None => {}
        }
    }
    None
}

/// The prompt a human typed in terminal `sid`, since its Claude conversation began, that asks for goal
/// `id` to run, if there's one and no later prompt takes it back. "Run the goal" means `id` when it's the
/// conversation's goal: the terminal's task is in it, the conversation made a task in it, or (for the
/// prompts after it) the board handed it the goal.
pub fn owners_goal_word(app: &App, sid: &str, id: i64) -> Result<Option<String>> {
    if sid.is_empty() {
        return Ok(None);
    }
    let since = "COALESCE((SELECT MAX(id) FROM session_events WHERE session_id = ? AND kind = 'start'), 0)";
    let rows = app.db.q(
        &format!("SELECT id, text, full, data FROM session_events WHERE session_id = ? AND kind = 'prompt' AND id > {since} ORDER BY id DESC"),
        p![sid, sid],
    )?;
    let board = |r: &Row| r.s("data") == Some(BOARD_PROMPT);
    // Newest first: the prompts after the board's handoff, if any, come before it in `rows`.
    let handed = rows.iter().position(|r| board(r) && MARKER_RE.find_iter(&typed(r).0).any(|m| ids_in(&GOALS, m.as_str()).contains(&id)));
    let on_task = crate::board::task_for_session(app, Some(sid))?.and_then(|t| t.i("goal_id")) == Some(id);
    let made = app
        .db
        .q1(
            &format!(
                "SELECT 1 FROM session_events e JOIN tasks t ON t.id = CAST(e.data AS INTEGER)
                  WHERE e.session_id = ? AND e.kind = ? AND e.id > {since} AND t.goal_id = ? LIMIT 1"
            ),
            p![sid, MADE, sid, id],
        )?
        .is_some();
    let prompts: Vec<(Prompt, bool)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let (text, clipped) = typed(r);
            (Prompt { text, made: vec![], clipped, board: board(r) }, on_task || made || handed.is_some_and(|h| i < h))
        })
        .collect();
    Ok(goal_word_in(&prompts, id).map(|i| prompts[i].0.text.clone()))
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
    fn a_prompt_whose_end_never_came_is_no_word_but_takes_back_only_what_it_says() {
        let p = |text: &str, clipped| Prompt { clipped, ..Prompt::new(text) };
        assert_eq!(word_in(&[p("start T8", true)], 8), None, "its unread end may take the start back");
        assert_eq!(word_in(&[p("thanks", true), p("start T8", false)], 8), Some(1));
        assert_eq!(word_in(&[p("don't start T8", true), p("start T8", false)], 8), None);
        assert_eq!(word_in(&[p("start T8", false)], 8), Some(0));
    }

    #[test]
    fn a_long_prompt_keeps_its_start_and_its_end() {
        let paste = "ERROR something broke at line 12\n".repeat(900);
        let long = format!("start T8. here's the log:\n{paste}\nok, that's all");
        let kept = keep_ends(&long, 20_000);
        assert!(kept.chars().count() <= 20_000);
        assert!(kept.starts_with("start T8. here's the log:\n") && kept.ends_with("ok, that's all"), "{kept}");
        assert!(kept.contains(CUT_MARK));
        assert_eq!(keep_ends("start T8", 20_000), "start T8");
        // The board reads both ends: the ask at the start, a take-back at the end.
        assert_eq!(word_in(&[Prompt::new(&kept)], 8), Some(0));
        let back = keep_ends(&format!("start T8. here's the log:\n{paste}\njk"), 20_000);
        assert_eq!(word_in(&[Prompt::new(&back)], 8), None);
        let asked = keep_ends(&format!("here's the log:\n{paste}\nok, start T8"), 20_000);
        assert_eq!(word_in(&[Prompt::new(&asked)], 8), Some(0));
        // A paste cut in the middle is still a paste on both sides of the cut.
        let pasted = keep_ends(&format!("here's the log:\n{}start T8\n", paste.replace('\n', " | ")), 20_000);
        assert_eq!(word_in(&[Prompt::new(&pasted)], 8), None, "{pasted}");
        let said = keep_ends(&format!("start T9. the log says: {}start T8", "x ".repeat(12_000)), 20_000);
        assert_eq!(word_in(&[Prompt::new(&said)], 8), None);
        assert_eq!(word_in(&[Prompt::new(&said)], 9), Some(0));
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

    #[test]
    fn a_goal_run_is_asked_for_by_name_or_as_the_goal() {
        let goal = |t: &str| read_of(&GOALS, t);
        for yes in ["start G1", "run G1", "ok, run the goal", "start the goal", "kick off G1", "please queue goal G1", "run the goal G1 now", "You can start the goal"] {
            assert!(!goal(yes).asks.is_empty(), "{yes}");
        }
        for no in [
            "run it",
            "run the tests",
            "start T1",
            "did you start G1?",
            "don't run G1",
            "run G1 tomorrow",
            "run G1 once T4 lands",
            "the log says: run G1",
            "G1 started an hour ago",
            "how do I run the goal",
            "run G1. jk",
        ] {
            assert!(goal(no).asks.is_empty(), "{no}");
        }
        assert_eq!(goal("run G1 and G2").asks, vec![Ask::Named(vec![1, 2])]);
        assert_eq!(goal("start the goal G3").asks, vec![Ask::Named(vec![3])]);
        // A goal ask is no word for a task, and the other way around.
        assert!(!says_start("start G1"));
        assert!(!says_start("start the goal"));
    }

    #[test]
    fn the_latest_prompt_on_a_goal_run_decides() {
        let word = |ps: &[(&str, bool)], id| goal_word_in(&ps.iter().map(|(t, o)| (Prompt::new(t), *o)).collect::<Vec<_>>(), id);
        assert_eq!(word(&[("start G1", false)], 1), Some(0));
        assert_eq!(word(&[("start G1", false)], 2), None);
        assert_eq!(word(&[("thanks", false), ("run G1", false)], 1), Some(1));
        assert_eq!(word(&[("wait, don't run G1", false), ("run G1", false)], 1), None);
        assert_eq!(word(&[("hold off on G1", false), ("run G1", false)], 1), None);
        assert_eq!(word(&[("wait", false), ("run G1", false)], 1), None);
        assert_eq!(word(&[("start the goal", false)], 1), None, "not this conversation's goal");
        assert_eq!(word(&[("start the goal", true)], 1), Some(0));
        assert_eq!(word(&[("[task-board:G1] Plan the goal. Run G1.", true)], 1), None);
        let clipped = Prompt { clipped: true, ..Prompt::new("run G1 …") };
        assert_eq!(goal_word_in(&[(clipped, false)], 1), None, "its unread end may take it back");
    }
}
