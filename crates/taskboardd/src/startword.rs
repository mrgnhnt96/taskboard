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

/// `session_events.data` on a prompt whose hook said it cut only with [`keep_ends`] (`prompt_cut`): it
/// came whole or with both ends, so a "…" at its end is the owner's, not the old hook's cut.
pub const KEPT_ENDS: &str = "ends";

/// How much of the end of the agent's last message the Stop hook sends (`last_message_end`), for
/// [`asks_to_run`].
pub const REPLY_END_KEEP: usize = 1000;
/// The hook before this kept only the first 2000 characters of the agent's message: one that long may
/// have lost its end.
const OLD_HOOK_REPLY_KEEP: usize = 2000;

/// What the UserPromptSubmit hook reports of a typed prompt: the prompt, whole up to what the board
/// keeps or cut with [`keep_ends`], and that it was cut only that way.
pub fn hook_prompt(prompt: &str) -> serde_json::Value {
    serde_json::json!({"prompt": keep_ends(prompt, crate::board::PROMPT_FULL_KEEP), "prompt_cut": KEPT_ENDS})
}

/// `session_events.kind` for a task the terminal's conversation made (`tb task new`); `data` is its id.
pub const MADE: &str = "added";
/// `session_events.kind` for a goal run on the owner's word in that terminal (`data` the goal's ref):
/// the word it ran on is spent.
pub const RAN: &str = "ran";
/// `session_events.text` of the `start` a `/clear` leaves.
pub const CLEARED: &str = "Cleared its conversation and started again";

/// Any board marker (`[task-board:T4]`, `[task-board:G2]`, `[task-board:J7]`, …): the prompt is the board's.
pub static MARKER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[task-board:[^\]\n]*\]").unwrap());

/// Where an ask can begin: the start of a sentence or of a clause.
const EDGE: &str = r"(?:^|[:,(\[—–]|\s-+\s|\b(?:and|then|but|so|also)\b)";
/// What may come before the start word in an ask ("ok,", "please", "actually", "go ahead and", "can you").
const LEAD: &str = r"(?:(?:ok(?:ay)?|alright|all\s+right|yes|yeah|yep|sure|great|cool|perfect|lgtm|looks\s+good|sounds\s+good|thanks|please|pls|now|then|so|also|just|actually|go|go\s+ahead(?:\s+and)?|you\s+can|you\s+may|(?:can|could|would|will)\s+(?:you|u)|i\s+want\s+you\s+to|i[’']?d\s+like\s+you\s+to|let[’']?s|let\s+us|feel\s+free\s+to|time\s+to)\s*,?\s+)*";
/// The tasks an ask names: "T4", "task T4", "T4, T5 and T6".
const NAMED: &str = r"(?:task\s+)?[Tt]\d+(?:\s*,?\s*(?:and\s+|&\s*|\+\s*)?(?:task\s+)?[Tt]\d+)*";
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
    Regex::new(r"(?i)(?:^|[.!?;\n,:(—–]\s*|\b(?:and|so|but|ok(?:ay)?|i\s+(?:think|guess|reckon|feel\s+like))\s+)(?:task\s+)?[Tt](\d+)\s*(?:(?:is|[’']s|looks|seems|sounds|has\s+been)\b|[:—–]|-+\s)").unwrap()
});
/// A prompt that's nothing but an unnamed ask ("queue it", "ok, start them", "great, kick it off"): its
/// "it" can only be what the conversation just made. Asking to hear back ("queue it and let me know when
/// it's done") says nothing more about "it".
static BARE_ASK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)^(?:(?:ok(?:ay)?|alright|all\s+right|yes|yeah|yep|yup|sure|great|cool|nice|perfect|awesome|good|thanks|thank\s+you|ty|please|pls|looks\s+good|lgtm|sounds\s+good|go\s+ahead(?:\s+and)?|now|then|so|and|just|you\s+can|can\s+you|could\s+you|let[’']?s|that[’']?s\s+(?:fine|great|good|perfect))[\s,.!]*)*(?:{VERB}\s+(?:{UNNAMED})|kick\s+(?:{UNNAMED})\s+off|get\s+(?:{UNNAMED})\s+going|(?:spin|fire)\s+(?:{UNNAMED})\s+up)(?:\s+(?:up|off|now|please|pls|too|for\s+me|right\s+away|right\s+now|asap))*(?:[\s,]*(?:and\s+)?(?:let\s+me\s+know|tell\s+me|ping\s+me|lmk)\b[^.!?;\n]*)?[\s.!]*(?:(?:thanks|thank\s+you|ty|please|pls)[\s.!]*)?$"
    ))
    .unwrap()
});
/// What may follow the task in an ask: the end of the clause, or a word that goes on with it ("start T8
/// without the migration", "start T8 on Monday"). Anything else ("start T8 failed", "start T8 is
/// broken") makes it no ask. A time, a condition or a question is read with the clause and the sentence.
const TAIL: &str = concat!(
    r"(?:\s*$|\s*[:,()\[\]—–]|\s+-|\s+(?:and|then|now|please|pls|too|again|up|off|asap|instead|for|right\s+away|right\s+now|",
    r"without|with|but|so|also|or|not|first|next|immediately|today|tomorrow|tonight|later|on|in|at|by|after|before|when|whenever|",
    r"once|if|unless|until|till|as|this|down|around|during|thanks|thank|thx|ty|it|it[’']s|its|from|using|via|to|while|since|because|anyway|jk)\b)"
);
/// The start words.
const VERB: &str = r"(?:start|strat|satrt|queue|begin|launch|kick\s+off|get\s+going)(?:\s+up)?(?:\s+(?:work(?:ing)?\s+)?on)?";
/// The start words that ask only with a task named ("run T8", "pick up T8", "go ahead with T8", "do
/// T8", "spin up T8", "work on T8", "go T8", but not "run it" or "do it").
const NAMED_VERB: &str = r"(?:run|pick\s+up|do|go\s+ahead\s+with|go\s+for|fire\s+off|fire\s+up|spin\s+up|work\s+on|go)";
/// A task named and then the word to go, as the whole clause ("T8 go", "T8: go"). Only the owner's: in
/// the agent's question it asks nothing ("Should I close T8 now?", [`asked_in`]).
const NAMED_GO: &str = r"\s*[:,—–-]?\s*(?:go(?:\s+ahead)?|next|now)(?:\s+(?:now|please|pls))?\s*$";
/// An ask to put one in the queue ("put T8 in the queue", "add it to the queue").
const QUEUE_PUT: &str = r"(?:put|add|send|stick|pop)";
const QUEUE_TO: &str = r"(?:in(?:to)?|on(?:to)?|to)\s+(?:the\s+)?queue";

/// The goals an ask names: "G2", "goal G2", "the goal G2", "G2 and G3".
const NAMED_GOAL: &str = r"(?:the\s+)?(?:goal\s+)?[Gg]\d+(?:\s*,?\s*(?:and\s+|&\s*)?(?:goal\s+)?[Gg]\d+)*";
/// The goal an ask doesn't name: "the goal", "this goal", "the whole goal", "all the tasks", "all of
/// them", or "it" ([`Ask::It`]), which is the goal only where [`GoalPrompt::it`] says so.
const UNNAMED_GOAL: &str = r"(?:(?:all|each)\s+of\s+(?:them|these|those|its\s+tasks)|them\s+all|(?:the|this|that|my|your|our|its)\s+(?:whole\s+|new\s+)?goal|all\s+(?:of\s+)?(?:the\s+|its\s+|these\s+|those\s+|your\s+|my\s+)?tasks|every\s+task|everything(?:\s+in\s+(?:the|this|that|my|your|our)\s+goal)?|both(?:\s+of\s+(?:them|these|those))?|it|this|that|them)";
/// An unnamed goal ask that says only "it" ("run it", "kick it off").
static IT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:it|this|that|them)$").unwrap());
/// The start words for a goal, which also runs and ships ("ship it").
const GOAL_VERB: &str = r"(?:start|queue|begin|launch|kick\s+off|run|ship|get\s+going|go\s+ahead\s+with)(?:\s+up)?(?:\s+(?:work(?:ing)?\s+)?on)?";

/// What a start is read on: tasks (`tb start T4`) or goals (`tb start G2`).
pub struct Kind {
    /// A start word at the start of a clause, on one, with nothing after it that makes it a story.
    ask: Regex,
    /// A start word on one anywhere: an ask, or (when it's no ask) a mention that takes the start back.
    mention: Regex,
    /// One named, with its number.
    ids: Regex,
    /// Goals: an unnamed "it" is [`Ask::It`].
    goal: bool,
}

fn kind(named: &str, unnamed: &str, verb: &str, named_verb: Option<&str>, ids: &str, goal: bool) -> Kind {
    let mut on = format!(
        r"(?:\b{verb}\s+(?:{LISTED}(?P<named>{named})|(?P<unnamed>{unnamed}))|\bkick\s+(?:(?P<named2>{named})|(?P<unnamed2>{unnamed}))\s+off|\bget\s+(?:(?P<named6>{named})|(?P<unnamed6>{unnamed}))\s+going|\b(?:spin|fire)\s+(?:(?P<named7>{named})|(?P<unnamed7>{unnamed}))\s+up"
    );
    if let Some(v) = named_verb {
        on.push_str(&format!(r"|\b{v}\s+{LISTED}(?P<named3>{named})|(?P<named5>\b{named}){NAMED_GO}"));
    }
    on.push_str(&format!(r"|\b{QUEUE_PUT}\s+(?:{LISTED}(?P<named4>{named})|(?P<unnamed4>{unnamed}))\s+{QUEUE_TO}"));
    on.push(')');
    Kind { ask: Regex::new(&format!(r"(?i){EDGE}\s*{LEAD}{on}{TAIL}")).unwrap(), mention: Regex::new(&format!(r"(?i){on}\b")).unwrap(), ids: Regex::new(ids).unwrap(), goal }
}

/// Starts of tasks.
pub static TASKS: Lazy<Kind> = Lazy::new(|| kind(NAMED, UNNAMED, VERB, Some(NAMED_VERB), r"\b[Tt](\d+)\b", false));
/// Runs of goals.
pub static GOALS: Lazy<Kind> = Lazy::new(|| kind(NAMED_GOAL, UNNAMED_GOAL, GOAL_VERB, None, r"\b[Gg](\d+)\b", true));
/// A sentence and what ends it.
static SENTENCE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?P<s>[^.!?;\n…]*)(?P<end>[.!?;\n…]*)").unwrap());
/// Dotted words whose dots end no sentence ("6 a.m.", "e.g.").
static DOTTED_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b([ap])\.m\.|\b(e)\.g\.|\b(i)\.e\.").unwrap());
/// Where a clause of a sentence ends and the next begins: a comma, a colon, a bracket or a dash, or a
/// joining word ("and", "then", "but") that doesn't join two tasks ("T8 and T9").
static CLAUSE_EDGE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)[,:()\[\]{}—–]|\s-+\s|\b(?:and|then|but|so|or|also|plus|while)\b").unwrap());
/// What follows a joining word that joins two tasks, not two clauses…
static JOINS_TASKS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^\s*(?:task\s+|goal\s+)?[TtGg]\d+\b").unwrap());
/// …when a task comes right before it too ("T8 and T9", but not "start T8 now and T9 tomorrow").
static TASK_BEFORE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b[TtGg]\d+\s*,?\s*$").unwrap());
/// "A" before "while", which makes it a time ("in a while"), not a joining word.
static A_BEFORE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\ba\s+$").unwrap());
/// What may come before a dash an ask follows and still leave it the owner's even when a second dash
/// closes it ("ok - start T8 - it's ready").
static DASH_LEAD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s_]*(?:(?:ok(?:ay)?|alright|all\s+right|yes|yeah|yep|yup|sure|great|cool|nice|perfect|good|fine|awesome|thanks|thank\s+you|ty|please|pls|right|so|and|also|now|then|lgtm|looks\s+good|sounds\s+good|got\s+it)[\s,!.]*)*$").unwrap()
});
/// What before a dash makes what follows it someone else's words: a source ("the ticket reads — start
/// T8", "copied from CI - start T8", "CI output - start T8"). Anything else ("tests pass - start T8",
/// "hey - start T8") is the owner's lead-in.
static DASH_SOURCE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:says?|said|reads?|wrote|writes|output|outputs|copied|pasted|quoted?|quotes|log|logs|logged|prints?|printed|shows?|showed|stdout|stderr|trace|traceback|error|message|comment|ticket|ci|jira|slack|email|thread|transcript|docs?|document|readme|handoff|notes|spec|wiki|runbook|changelog)\b").unwrap()
});
/// What after a source word says something of it, which makes it the owner's lead-in ("CI's green -
/// start T8", "the ticket is done — start T8", "CI passed - start T8").
static STATED_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(?:(?P<be>[’']s\b|\s+(?:is|are|was|were|has|have|had)\b)|\s+(?:passed|passes|pass|looks|seems|went|turned|came\s+back)\b)").unwrap()
});
/// Whether `after`, what follows a source word, says something of it ([`STATED_RE`]). A "was" or an
/// "'s" with nothing said after it hands over the words after the dash ("the comment was - start T8").
fn stated(after: &str) -> bool {
    STATED_RE.captures(after).is_some_and(|c| c.name("be").is_none_or(|be| after[be.end()..].contains(char::is_alphanumeric)))
}
/// A choice in a question ("would you start T8 or T9 first?"), which makes it no polite ask.
static CHOICE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bor\b").unwrap());
/// "Would you mind starting T8?", read as "could you start T8?".
static MIND_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:would|do|will)\s+(?:you|u)\s+mind\s+(?P<v>start|queu|launch|begin|kick)(?:e|n)?ing\b").unwrap()
});
/// What before a start word makes a question a polite ask ("can you start T8?", "could you please
/// queue it?"), not a question about it.
static POLITE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s_,]*(?:(?:hey|so|ok(?:ay)?|and|also|then|now|please|pls)[\s,]+)*(?:can|could|would|will)\s+(?:you|u)\s+(?:(?:please|pls|just|maybe|go\s+ahead\s+and)\s+)*$").unwrap()
});
/// A clause that's a greeting, not a time ("morning", "good evening").
static GREETING_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s_]*(?:(?:hi|hey|hello|yo|gm|good\s+(?:morning|afternoon|evening|day)|morning|afternoon|evening)(?:\s+(?:all|everyone|team|there|again))?[\s!,.]*)+$").unwrap()
});
/// A no that points back at what was said before it ("don't do that", "not that one"), which isn't a no
/// on the start after it ("don't do that, start T8 instead").
static NO_BACK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:not|never|don[’']?t|do\s+not)\s+(?:do\s+)?(?:that|this|it)(?:\s+(?:one|either|again))?\s*[!.]*\s*$").unwrap()
});
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
    let fill = r"(?:please|pls|ok(?:ay)?|i|you|we|just|really|seriously|ever|again|now|yet|repeat|but|and|so|actually|though|tho|at\s+all|_|under\s+any\s+circumstances|whatever\s+you\s+do|for\s+any\s+reason|no\s+matter\s+what|want|wanna|need|think|would|like|to|it|that|this|do|(?:task\s+)?[TtGg]\d+)";
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
        r"(?:work|working|business|office|school|core|waking|opening|off|after)[\s-]+hours|(?:around|about|past)\s+\d|",
        // "During" only with a time after it: "during the run, keep an eye on CI" puts nothing off.
        r"during\s+(?:the\s+|my\s+|your\s+|our\s+|this\s+|that\s+|next\s+|a\s+)?(?:[a-z]+\s+)?(?:hours?|day|days|daytime|night|nights|week|weekend|morning|afternoon|evening|lunch|dinner|break|standup|stand-up|meeting|call|demo|sync|holidays?|vacation|weekdays?|sprint|deploy|freeze|window|downtime)|",
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
static NOT_A_TIME_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:not|rather\s+than|instead\s+of)\s+(?:later|tomorrow|tonight|afterwards?|next\s+\w+|waiting)\b|\bas\s+soon\s+as\s+possible\b|",
        // "Start T8 during the standup is fine": the time is said to be fine, now included.
        r"\bduring\s+(?:the\s+|my\s+|your\s+|our\s+|this\s+)?[a-z-]+\s+(?:is|[’']s)\s+(?:fine|ok(?:ay)?|good)\b|",
        // "When you're ready" is now: the agent is.
        // Only with nothing after it but the clause's end: "when you can confirm T9 passes" is a condition.
        r"\bwhen(?:ever)?\s+(?:you\s+(?:get\s+a\s+(?:chance|sec(?:ond)?|moment|minute)|can|have\s+(?:a\s+)?(?:sec(?:ond)?|moment|minute|chance))|you[’']?re\s+(?:ready|free)|you\s+are\s+(?:ready|free))",
        r"(?:\s+(?:please|pls|thanks|thank\s+you|thx|ty))*\s*(?:$|[.!?,;:()\[\]—–]|\s-)"
    ))
    .unwrap()
});
/// What may come before the time or condition in a clause that's only that ("on the 15th", "only
/// after T7 lands", "then"), so it falls on a start next to it.
static ONLY_LEAD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s_]*(?:(?:and|but|so|or|also|then|just|actually|ok(?:ay)?|maybe|probably|ideally|preferably|say|i\s+mean|um+|uh+|on|at|by|in|for|around|about|from|the|some|only|even|please|no\s+earlier\s+than|not|over|during|within|through|throughout|outside|as|asap|right|top|bottom|middle|start|beginning|of)\s+)*$").unwrap()
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
        r"(?i)\b(?:wait|hold\s+(?:off|on|up)|hold\s+(?:that|this)(?:\s+thought)?|never\s*mind|nvm|scratch\s+that|cancel\s+(?:that|it|this|them|these|those)|",
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
    Lazy::new(|| Regex::new(r"(?i)\b(?:then|and|no\s+need\s+to|(?:don[’']?t|do\s+not|doesn[’']?t)(?:\s+need\s+to)?|never|without|the|a|long|this|that|your|my)\s*$").unwrap());
/// A sentence that ends on a no ("on second thought, don't", "please don't", "I'd rather you not").
static ENDS_NO_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:don[’']?t|do\s+not|not|never)\s*(?:please|pls|yet|now|though|tho|after\s+all|anymore|any\s+more)?\s*[,!]*\s*$").unwrap()
});
/// A sentence that starts with a bare no ("no.", "nope", "no, don't", "nah"), not one that starts with a
/// no on something ("no need to ask", "no rush").
static STARTS_NO_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s,]*(?:(?:oh|ah|um+|uh+|hm+|lol|lmao|ha(?:ha)+h?|hah|heh|oops|whoops|ugh+|actually|wait|ok(?:ay)?|well|so|and|but)\b[\s,]*)*(?P<no>(?:no+|nope|nah|naw|nay|negative)(?:[\s,]+(?:no+|nope|nah))*)\s*(?:[,!]|$)").unwrap()
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
    Regex::new(concat!(
        r"(?i)\b(?:make|create|add|file|open|write|set\s+up|log|draft|a\s+new)\b[^.!?;\n]*\b(?:task|ticket|card|one)s?\b|",
        r"\b(?:put|add|get)\b[^.!?;\n]*\b(?:on|onto|to)\s+the\s+board\b|\btrack\b[^.!?;\n]*\bas\s+(?:a\s+)?(?:new\s+)?(?:task|ticket)s?\b"
    ))
    .unwrap()
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
    /// A goal ask that says only "it" ("run it"): what it means depends on what came before.
    It,
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

/// A task or goal ref as inline code ("`T8` - start it"): the ref, not code.
static CODE_REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`\s*((?:task\s+|goal\s+)?[TtGg]\d+)\s*`").unwrap());
/// Bold or italic marks around words ("**start T8**", "__T8__", "*now*").
static EMPHASIS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*\*([^*\n]+)\*\*|__([^_\n]+)__|(^|[\s(])\*([^*\s][^*\n]*?)\*").unwrap());
/// A markdown heading the owner wrote ("## Plan"): the list under it is theirs too.
static HEADING_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s{0,3}#{1,6}\s+\S").unwrap());
/// A list item's mark ("- ", "* ", "1. ").
static BULLET_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*(?:[-*•+]|\d{1,3}[.)])\s+").unwrap());
/// The emoji that say yes or go ("👍", "✅", "🚀"), with a skin tone or the emoji form after them.
static YES_EMOJI_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:[👍👌✅✔☑🚀🙏🎉🙌💪💯🔥✨🥳🟢⚡]|:\+1:|:shipit:)[\u{1F3FB}-\u{1F3FF}\u{FE0F}]*").unwrap());
/// Those emoji at the start of a line, and the space after them.
static LEAD_EMOJI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(&format!(r"(?m)^[ \t]*(?:(?:{})[ \t]*)+", YES_EMOJI_RE.as_str())).unwrap());
/// A prompt that's only those emoji.
static ONLY_EMOJI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[\s!.,]*$").unwrap());
/// Everyday short forms, read as the words they stand for ("plz", "u", "em", "w/").
static SHORT_FORM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:plz|plox|pleaze|u|em|w/|thx)(?:\b|\s|$)|[’']em\b").unwrap());
/// "Kick of", a slip of "kick off" ("kick of T8", "kick it of").
static KICK_OF_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(kick(?:\s+(?:(?:task\s+|goal\s+)?[TtGg]\d+|it|this|that|them|these|those|both))?\s+)of\b").unwrap());
/// A word, for [`fix_typo`].
static TYPO_WORD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[A-Za-z]+").unwrap());
/// The start and yes words a one-letter slip is read as ("sart", "ahaed", "pleae"), and the real words
/// a slip of them can be, which stay ("star", "smart", "begun").
const TYPO_WORDS: &[&str] = &["start", "ahead", "queue", "please", "launch", "begin", "going"];
const NOT_TYPOS: &[&str] = &[
    "star", "stars", "starts", "tart", "smart", "stark", "stare", "stat", "stats", "stuart", "head", "ahem", "began", "begun", "being",
    "beg", "begins", "lease", "plead", "pleat", "please", "paunch", "launchd", "haunch", "queues", "queued", "doing", "goings",
    "gong", "gaining", "owing", "going", "lunch", "pleased", "pleases", "staring", "boing", "start", "ahead", "queue", "launch", "begin",
];

/// Whether `a` is `b` with one slip: a letter left out, added, changed, or two swapped.
fn one_slip(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a == b || a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let (x, y) = (&a[pre..], &b[pre..]);
    match x.len().cmp(&y.len()) {
        std::cmp::Ordering::Equal => x[1..] == y[1..] || (x.len() >= 2 && x[0] == y[1] && x[1] == y[0] && x[2..] == y[2..]),
        std::cmp::Ordering::Less => x == &y[1..],
        std::cmp::Ordering::Greater => &x[1..] == y,
    }
}

/// The word `w` stands for when it's a slip of a start or yes word ("sart", "yse", "yess", "teh").
fn fix_typo(w: &str) -> Option<&'static str> {
    let l = w.to_lowercase();
    match l.as_str() {
        "yse" | "eys" | "yeas" | "yees" | "yesh" | "yrs" => return Some("yes"),
        "teh" | "hte" => return Some("the"),
        "yeha" | "yaeh" | "yeh" => return Some("yeah"),
        "ok" | "okay" => return None,
        _ => {}
    }
    if l.len() > 3 && l.starts_with("yes") && l[3..].chars().all(|c| c == 's') {
        return Some("yes");
    }
    if l.len() < 4 || NOT_TYPOS.contains(&l.as_str()) {
        return None;
    }
    TYPO_WORDS.iter().copied().find(|t| one_slip(&l, t))
}

/// `text` with the marks that dress words up taken off and slips read as meant: a ref as inline code,
/// bold and italics, a heading's "#" and the bullets of the list under it, emoji that say yes (only those,
/// "yes"; else nothing), short forms ("plz", "u", "em", "w/") and one-letter slips of the start and yes
/// words ([`fix_typo`]).
fn plain(text: &str) -> String {
    let text = CODE_REF_RE.replace_all(text, "$1");
    let text = EMPHASIS_RE.replace_all(&text, "$1$2$3$4");
    let mut lines = vec![];
    let mut listed = false;
    for line in text.split('\n') {
        if HEADING_RE.is_match(line) {
            // The heading itself is a title, no ask ("# start T8" may be a shell comment).
            listed = true;
            lines.push(String::new());
        } else if listed && BULLET_RE.is_match(line) {
            lines.push(BULLET_RE.replace(line, "").to_string());
        } else {
            listed &= line.trim().is_empty();
            lines.push(line.to_string());
        }
    }
    let text = lines.join("\n");
    let text = if YES_EMOJI_RE.is_match(&text) && ONLY_EMOJI_RE.is_match(&YES_EMOJI_RE.replace_all(&text, "")) {
        "yes".to_string()
    } else {
        // A line of only them is a yes ("> Should I start T8?\n👍"); at a line's start they leave no
        // indent behind (an indented line reads as pasted).
        let only = |l: &str| YES_EMOJI_RE.is_match(l) && ONLY_EMOJI_RE.is_match(&YES_EMOJI_RE.replace_all(l, ""));
        let text: Vec<String> = text.split('\n').map(|l| if only(l) { "yes".to_string() } else { l.to_string() }).collect();
        let text = LEAD_EMOJI_RE.replace_all(&text.join("\n"), "").to_string();
        YES_EMOJI_RE.replace_all(&text, " ").to_string()
    };
    let text = SHORT_FORM_RE.replace_all(&text, |c: &regex::Captures| {
        let m = &c[0];
        let tail = &m[m.trim_end().len()..];
        let word = match m.trim_end().to_lowercase().as_str() {
            "plz" | "plox" | "pleaze" => "pls",
            "u" => "you",
            "w/" => "with",
            "thx" => "thanks",
            _ => "them",
        };
        format!("{word}{tail}")
    });
    let text = KICK_OF_RE.replace_all(&text, "${1}off");
    TYPO_WORD_RE.replace_all(&text, |c: &regex::Captures| fix_typo(&c[0]).map_or_else(|| c[0].to_string(), |w| w.to_string())).to_string()
}

/// The owner's own words in `text`: code, quotes, the rest of a line after "says:", the lines after a
/// line that ends in a colon (up to a blank line) or that hands over output ("here's the log."), and
/// lines that read as pasted all come out. Read [`plain`].
pub fn owners_text(text: &str) -> String {
    let text = plain(&text.replace("\r\n", "\n"));
    // The middle of a long prompt, cut on its way in: no words, but a paste goes on through it.
    let text = text.replace(CUT_MARK, "…");
    let text = CODE_RE.replace_all(&text, "\n");
    // A quote is no clause edge: the words around it stay one sentence (and keep their "don't").
    let text = QUOTE_RE.replace_all(&text, " _ ");
    let mut out = vec![];
    // After a line that hands over a paste ("CI failed with:"), the paste: from the next line that
    // isn't blank to the next blank line.
    let (mut pasted, mut lead) = (false, false);
    let lines: Vec<&str> = text.split('\n').collect();
    for (i, line) in lines.iter().copied().enumerate() {
        if pasted {
            if line.trim().is_empty() && lead {
                out.push(String::new());
                continue;
            }
            // The paste's last line, when it takes back what came before ("…\njk"), is the owner's.
            let ends = lines.get(i + 1).is_none_or(|l| l.trim().is_empty());
            if !lead && ends && takes_back(line) {
                pasted = false;
                out.push(line.to_string());
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

/// A line that's only a take-back ("jk", "never mind", "actually, don't"), not one that goes on into other
/// words ("Wait timeout exceeded", "Hold on, retrying in 5s", "never mind the warnings above").
static BACK_LINE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)^[\s,.!]*(?:(?:ok(?:ay)?|oh|ah|um+|uh+|so|well|actually|please|lol|lmao|ha(?:ha)+h?|hah|heh|hehe|hm+|ugh+|oops|whoops|welp|wait|hold\s+(?:on|off|up)|jk|j/k|just\s+(?:kidding|joking)|kidding|joking|psych|",
        r"never\s*mind|nvm|scratch\s+that|forget\s+(?:it|that|this)|forget\s+(?:what\s+)?i\s+said(?:\s+that)?|cancel\s+(?:that|it|this)|don[’']?t|do\s+not|",
        r"no+|nope|nah|not\s+(?:yet|now)|not\s+(?:that|this|it)(?:\s+one)?|on\s+second\s+thoughts?|(?:i\s+)?changed\s+my\s+mind|ignore\s+(?:that|this|it)|let[’']?s\s+not|take\s+(?:it|that)\s+back)[\s,.!]*)+$"
    ))
    .unwrap()
});

/// A line that starts with a take-back (after "lol", "hmm", "ugh") and goes on with a no or a wait
/// ("never mind, don't start it", "hold off on that", "actually let's wait on that", "wait no, I need to
/// check something first"), not one that goes on with something else ("Hold on, retrying in 5s").
static BACK_ON_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)^[\s,.!]*(?:(?:lol|lmao|ha(?:ha)+h?|hah|heh|hm+|ugh+|oh|ah|um+|uh+|ok(?:ay)?|so|well|oops|whoops|welp)[\s,.!]+)*",
        r"(?:wait|hold\s+(?:on|off|up)|never\s*mind|nvm|scratch\s+that|actually|on\s+second\s+thoughts?|jk|just\s+kidding)[\s,.!—–-]+",
        // A "no" or "not" that says no, not one before what it's about ("Wait - no response from the host").
        r"(?:don[’']?t|do\s+not|(?:no+|nope|not)(?:\s*(?:$|[,.!—–-])|\s+(?:i|we|let[’']?s|don[’']?t|do|not|wait|hold|stop|that|this|it|yet|now|so|actually|never)\b)|",
        r"let[’']?s\s+(?:not|wait|hold)|on\s+(?:that|it|this)\b|wait\b|hold\s+(?:off|on)\b)"
    ))
    .unwrap()
});

/// A line that takes back what came before it, and is nothing else ([`BACK_LINE_RE`]) or goes on only
/// with a no or a wait ([`BACK_ON_RE`]).
fn takes_back(line: &str) -> bool {
    let l = line.trim();
    let bare = l.trim_end_matches(['.', '!']);
    !PASTED_LINE_RE.is_match(line)
        && (BACK_LINE_RE.is_match(l) || BACK_ON_RE.is_match(l))
        && (HALT_RE.is_match(l) || STARTS_NO_RE.is_match(bare) || ENDS_NO_RE.is_match(bare) || NO_BACK_RE.is_match(l))
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

/// Whether mention `m` in `s` is a start word on something of a task's ("start T8's review", "run T8's
/// tests"), which says nothing on the task's own start.
fn of_a_tasks(s: &str, m: &Mention) -> bool {
    matches!(m.ask, Ask::Named(_)) && s[m.span.1..].starts_with(['\'', '’'])
}

/// The task span and the ask of one mention.
fn mention_of(k: &Kind, c: &regex::Captures<'_>) -> Option<Mention> {
    let verb = c.get(0)?.start();
    let soft = c.name("named3").or(c.name("named5"));
    if let Some(n) = c.name("named").or(c.name("named2")).or(c.name("named4")).or(c.name("named6")).or(c.name("named7")).or(soft) {
        return Some(Mention { verb, span: (n.start(), n.end()), ask: Ask::Named(ids_in(k, n.as_str())), soft: soft.is_some() });
    }
    let u = c.name("unnamed").or(c.name("unnamed2")).or(c.name("unnamed4")).or(c.name("unnamed6")).or(c.name("unnamed7"))?;
    let ask = if k.goal {
        if IT_RE.is_match(u.as_str()) {
            Ask::It
        } else {
            Ask::Unnamed
        }
    } else if FIRST_RE.is_match(u.as_str()) {
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
        if word && JOINS_TASKS_RE.is_match(&s[m.end()..]) && TASK_BEFORE_RE.is_match(&s[..m.start()]) {
            continue;
        }
        if m.as_str().eq_ignore_ascii_case("while") && A_BEFORE_RE.is_match(&s[..m.start()]) {
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

/// A no on a start word ("don't start", "do not queue")…
static BARE_DONT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:don[’']?t|do\s+not)\s+(?:start|queue|begin|launch|kick)").unwrap());
/// …and what follows it when it's on nothing ("don't start", "don't start until T9 merges", "don't start
/// yet"), not on something it names ("don't start T9", "don't start the migration", "don't start anything
/// else", "don't launch the emulator"), which holds only that.
static DONT_BARE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)^\w*(?:\s+(?:up|off))?\s*(?:$|[,.!;:?—–()-]|anything\s*(?:$|[,.!;?]|(?:yet|now|today|until|till|at\s+all|before|for\s+now)\b)|",
        r"(?:until|till|til|yet|now|today|tonight|tomorrow|later|before|after|unless|when|whenever|once|if|",
        r"though|tho|please|pls|just|right|again|at\s+all|any\s+of|for\s+now|so\s+fast|work(?:ing)?)\b)"
    ))
    .unwrap()
});

/// Where in sentence `s` it takes back every start before it, if it does: a take-back word with no task
/// after it in the sentence ("start T8, scratch that", "jk", "no.", "on second thought, don't").
fn halt_in(k: &Kind, s: &str) -> Option<usize> {
    let spans: Vec<(usize, usize)> = k.mention.captures_iter(s).filter_map(|c| mention_of(k, &c)).map(|m| m.span).collect();
    let refs: Vec<usize> = k.ids.find_iter(s).map(|m| m.start()).chain(spans.iter().map(|s| s.0)).collect();
    let last = |at: usize| !refs.iter().any(|r| *r >= at);
    let step = |m: &regex::Match| m.as_str().to_lowercase().starts_with(['w', 'h']) && STEP_BEFORE_RE.is_match(&s[..m.start()]);
    // A take-back in a clause about another task holds only that one ("start T8. T9 can wait").
    let cl = clauses(s);
    let elsewhere = |at: usize| {
        cl.iter().find(|(a, b)| *a <= at && at < *b).is_some_and(|(a, b)| {
            let rel: Vec<(usize, usize)> = spans.iter().filter(|(x, _)| x >= a).map(|(x, y)| (x - a, y - a)).collect();
            !about_in(k, &s[*a..*b], &rel).is_empty()
        })
    };
    // "Don't start" on nothing says no on every start ("start T8, don't start until T9 merges"), whatever it
    // names after; one on something ("don't start T9", "don't start anything else", "don't launch the
    // emulator") holds only that, which its own start word does.
    let halts = |m: &regex::Match| match BARE_DONT_RE.find(m.as_str()) {
        Some(d) => DONT_BARE_RE.is_match(&s[m.start() + d.end()..]),
        None => last(m.end()) && !elsewhere(m.start()),
    };
    let mut at: Vec<usize> = HALT_RE.find_iter(s).filter(|m| halts(m) && !step(m)).map(|m| m.start()).collect();
    at.extend(ENDS_NO_RE.find(s).filter(|m| last(m.end()) && !elsewhere(m.start())).map(|m| m.start()));
    if let Some(no) = STARTS_NO_RE.captures(s).and_then(|c| c.name("no")) {
        if last(no.end()) {
            at.push(no.start());
        }
    }
    if refs.is_empty() {
        at.extend(STOP_RE.find(s).map(|m| m.start()));
        // A sentence that's only a no on what came before it ("oops, not that one").
        if BACK_LINE_RE.is_match(s.trim()) && NO_BACK_RE.is_match(s) {
            at.push(0);
        }
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
    let own = MIND_RE.replace_all(&own, |c: &regex::Captures| {
        let v = c["v"].to_lowercase();
        format!("could you {}", if v == "queu" { "queue" } else { &v })
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
        // "Can you start T8?" asks for it; "should I start T8?" and "would you start T8 or T9 first?" ask
        // about it.
        let polite = k.mention.find(s).is_some_and(|m| POLITE_RE.is_match(&s[..m.start()])) && !CHOICE_RE.is_match(s);
        let asked = c.name("end").is_some_and(|m| m.as_str().contains('?')) && !polite;
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
        .filter(|m| ask_spans.contains(&m.span) || !of_a_tasks(s, m))
        .collect();
    // "start T9 instead": the asks before this one are off.
    if !mentions.is_empty() && INSTEAD_RE.is_match(&NOT_A_TIME_RE.replace_all(s, "")) {
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
            // A no on a task it names ("not T9 — start T8") or on what was said before ("don't do that,
            // start T8 instead") isn't a no on the start after it.
            let no = NO_CLAUSE_RE.is_match(t) && about_in(k, t, &[]).is_empty() && !NO_BACK_RE.is_match(t);
            if (no || heading) && starts_after(to) {
                pending = true;
            } else if only_a_time(t) && !GREETING_RE.is_match(t) && !THEN_RE.is_match(t) && about_in(k, t, &[]).is_empty() {
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
        // After a dash, an ask is someone else's words when what comes before the dash reads as their
        // source ("CI output - start T8"), or when a second dash closes it ("… — start T8 — weird").
        let before = s[..from].trim_end();
        let dash = before.ends_with(['—', '–']) || (before.ends_with('-') && before.trim_end_matches('-').ends_with(char::is_whitespace));
        let lead = before.trim_end_matches(['—', '–', '-']);
        let closed = s[to..].trim_start().starts_with(['—', '–', '-']);
        let source = DASH_SOURCE_RE.find_iter(lead).any(|m| !stated(&lead[m.end()..]));
        // "ok - start T8 - is what the doc says": the words after the closing dash name their source.
        let named_after = closed && DASH_SOURCE_RE.is_match(&s[to..]);
        let quoted = dash && ((!DASH_LEAD_RE.is_match(lead) && (source || closed)) || named_after);
        for m in mentions.iter().filter(|m| from <= m.verb && m.verb < to) {
            let shaped = ask_spans.contains(&m.span);
            if m.soft && !shaped && !asked && !timed && !pending {
                continue;
            }
            let clean = shaped && !asked && !pending && !timed && !quoted && negs_in(&s[from..m.verb]).is_empty();
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

/// The tasks clause `t` names outside its starts (`spans`) that it speaks of, with where: not one it
/// names only in a condition ("but T9 not until T8 merges" speaks of T9, not of T8).
fn about_in(k: &Kind, t: &str, spans: &[(usize, usize)]) -> Vec<(usize, i64)> {
    let cond = CONDITION_RE.find(t).map(|m| m.start()).unwrap_or(t.len());
    k.ids
        .captures_iter(t)
        .filter_map(|c| {
            let m = c.get(0)?;
            let inside = spans.iter().any(|(a, b)| *a <= m.start() && m.end() <= *b);
            (m.start() < cond && !inside).then(|| c[1].parse().ok().map(|id| (m.start(), id))).flatten()
        })
        .collect()
}

/// The tasks clause `t` (at `from` in its sentence) names outside its starts (`spans`), held when the
/// clause holds them off or puts them off ("hold off on T8", "T8 can wait", "not T9", "but T9 tomorrow").
fn loose_holds(k: &Kind, base: usize, from: usize, t: &str, spans: &[(usize, usize)], steps: &mut Vec<(usize, Step)>) {
    let loose: Vec<i64> = about_in(k, t, spans).into_iter().map(|(_, id)| id).collect();
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
    /// The end of the agent's message just before it, if the board has it: a bare "yes" answers the
    /// question it ends on ([`asks_to_start`]).
    pub reply: Option<String>,
}

impl Prompt {
    pub fn new(text: &str) -> Prompt {
        Prompt { text: text.to_string(), ..Prompt::default() }
    }
    fn boards(&self) -> bool {
        self.board || MARKER_RE.is_match(&self.text)
    }
}

/// How many prompts back a bare "queue it" looks for the tasks it means (the one that asked for them and
/// the quiet ones since).
const UNNAMED_REACH: usize = 3;
/// A prompt that only thanks or approves ("thanks", "looks right"): it asks for nothing "it" could be.
static ACK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(?:(?:ok(?:ay)?|alright|all\s+right|thanks|thank\s+you|thx|ty|great|nice|cool|perfect|awesome|good|fine|neat|lovely|looks\s+(?:good|great|right|fine|ok)|lgtm|sounds\s+(?:good|great|right)|got\s+it|right|yes|yeah|yep|sure|that[’']?s\s+(?:fine|great|good|right|perfect))[\s,.!]*)+$").unwrap()
});

/// The tasks an unnamed ask in the latest prompt, `prompts[0]`, can mean. In a prompt that asks for new
/// tasks, the ones made in reply to it: "make a task for it and queue it" means the first, "make tasks
/// for A and B and queue them" every one. In a prompt that's only the ask ("queue it", "ok, start them"),
/// the ones made in reply to the latest prompt that got some, when that one asked for them and the
/// prompts since ("thanks") said nothing else: "it" only when there's one. "The first one" is the first
/// of them either way. A prompt with more in it ("the dev server won't come up; start it") may mean
/// something else.
fn unnamed_tasks(prompts: &[Prompt], r: &Reading, ask: &Ask) -> Vec<i64> {
    let Some(p) = prompts.first() else { return vec![] };
    let first = |made: &[i64]| made.first().copied().into_iter().collect();
    if r.makes {
        return if *ask == Ask::Them { p.made.clone() } else { first(&p.made) };
    }
    // The latest prompt the conversation made tasks in reply to, and the prompts since, which made none
    // and only thanked or approved ("thanks", "looks right").
    let Some(back) = prompts.iter().skip(1).take(UNNAMED_REACH).position(|q| !q.made.is_empty()) else { return vec![] };
    let prev = &prompts[1 + back];
    let quiet = |q: &Prompt| !q.boards() && !q.clipped && ACK_RE.is_match(owners_text(&q.text).trim()) && read(&q.text).steps.is_empty();
    if !prompts[1..1 + back].iter().all(quiet) {
        return vec![];
    }
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
/// The latest prompt is also the word when it's a yes ([`says_yes`]) or only an unnamed ask ("start it")
/// to the agent's question about starting it ("Should I start T8?" "yes").
/// An unnamed ask ("queue it", "queue them") is the word only in the latest prompt, for the tasks
/// [`unnamed_tasks`] says it means. A prompt whose end never reached the board ([`Prompt::clipped`]) is
/// no word, since its end may take the ask back, but it takes back only what it says.
pub fn word_in(prompts: &[Prompt], id: i64) -> Option<usize> {
    for (i, p) in prompts.iter().enumerate() {
        if p.boards() {
            continue;
        }
        let r = read(&p.text);
        // The tasks the agent's question just before the latest prompt asks to start: what a yes, and a
        // bare "start it" or "start them", answers.
        let asked = if i == 0 { p.reply.as_deref().map(|b| asked_tasks(b, id)).unwrap_or_default() } else { vec![] };
        // The agent's question quoted in the prompt ("> Should I start T8?\nyes") asks it as well.
        let asked = if i == 0 && asked.is_empty() { quoted_ask(&p.text).map(|q| asked_tasks(&q, id)).unwrap_or_default() } else { asked };
        let bare = || BARE_ASK_RE.is_match(owners_text(&p.text).trim());
        if i == 0 && !p.clipped {
            let own = owners_text(&p.text);
            // "T9 instead" to "Should I start T8?": T9, and not T8.
            if INSTEAD_ONLY_RE.is_match(own.trim()) && p.reply.as_deref().is_some_and(|b| !asked_tasks(b, id).is_empty()) {
                return named_tasks(&own).contains(&id).then_some(0);
            }
            // "now" to "Should I start T8 now or after the release?".
            if r.steps.is_empty() && NOW_ONLY_RE.is_match(own.trim()) && p.reply.as_deref().and_then(now_of_choice).is_some_and(|q| asked_tasks(&q, id).contains(&id)) {
                return Some(0);
            }
        }
        let answers = |a: &Ask| match a {
            Ask::Them => asked.contains(&id),
            Ask::First => asked.first() == Some(&id),
            _ => asked == [id],
        };
        let unnamed = |a: &Ask| i == 0 && (unnamed_tasks(prompts, &r, a).contains(&id) || (answers(a) && bare()));
        match r.word_for(id, unnamed) {
            Some(true) if p.clipped => {}
            Some(true) => return Some(i),
            Some(false) => return None,
            // A yes may hold other tasks off as it goes ("yes, don't start T9").
            None if i == 0 && !p.clipped && only_others(&r) && asked.contains(&id) && says_yes(&TASKS, &p.text, id) => return Some(0),
            None if i == 0 && !p.clipped && r.steps.is_empty() && asked.contains(&id) && names_only(&p.text, id) => return Some(0),
            None => {}
        }
    }
    None
}

/// A prompt that only names tasks to start in place of the asked ones ("T9 instead", "no, T9 instead").
static INSTEAD_ONLY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(r"(?i)^(?:(?:no|nah|nope|actually|ok(?:ay)?|hmm+|just|do|start|go\s+with)[\s,.!]*)*{NAMED}\s+instead(?:[\s,]+(?:please|pls|thanks))*[\s.!]*$")).unwrap()
});
/// A prompt that's only "now" ("now", "now please", "right now"), the answer to a choice of when.
static NOW_ONLY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(?:(?:ok(?:ay)?|yes|yeah|yep|sure|let[’']?s\s+do|do\s+it|go)[\s,.!]*)*(?:right\s+)?now(?:[\s,.!]+(?:please|pls|thanks|is\s+(?:fine|good)))*[\s.!]*$").unwrap()
});
/// A question that offers a start now or later ("Should I start T8 now or after the release?"): up to its
/// "now".
static NOW_CHOICE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)^(?P<head>.*\bnow)\s*,?\s+or\s+[^.!?\n]*\?\s*$").unwrap());

/// The agent's question as one about the start now, from a question that offers it now or later.
fn now_of_choice(reply: &str) -> Option<String> {
    NOW_CHOICE_RE.captures(reply.trim_end()).map(|c| format!("{}?", &c["head"]))
}

/// A line of the prompt that quotes ("> …").
static QUOTE_LINE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*>\s?(.*)$").unwrap());

/// The question the prompt quotes from the agent before the owner's answer ("> Should I start T8?\nyes"):
/// the quoted lines, when they end on a question or an offer ("> I can start T8 now if you want.").
fn quoted_ask(prompt: &str) -> Option<String> {
    let lines: Vec<String> = prompt.lines().filter_map(|l| QUOTE_LINE_RE.captures(l).map(|c| c[1].to_string())).collect();
    let q = lines.join("\n");
    (q.trim_end().ends_with('?') || OFFER_RE.is_match(q.trim_end())).then_some(q)
}

/// Whether all `r` says is to hold off tasks it names (the asked one isn't among them, or it'd have said).
fn only_others(r: &Reading) -> bool {
    r.steps.iter().all(|s| matches!(s, Step::Hold(Ask::Named(_))))
}

/// Whether `prompt` only names tasks, task `id` among them ([`ONLY_NAMED_RE`]).
fn names_only(prompt: &str, id: i64) -> bool {
    let own = owners_text(prompt);
    ONLY_NAMED_RE.is_match(own.trim()) && named_tasks(&own).contains(&id)
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
    // A hook that says how it cut ([`hook_prompt`]) sent the prompt whole or with both ends.
    let kept = row.s("data") == Some(KEPT_ENDS);
    match row.s("full") {
        Some(full) => (full.to_string(), !kept && cut(full, OLD_HOOK_KEEP - 10..=OLD_HOOK_KEEP)),
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
        "SELECT id, kind, text, full, data FROM session_events WHERE session_id = ? AND kind IN ('prompt', 'reply', ?)
           AND id > COALESCE((SELECT MAX(id) FROM session_events WHERE session_id = ? AND kind = 'start'), 0)
         ORDER BY id",
        p![sid, MADE, sid],
    )?;
    let mut prompts: Vec<Prompt> = vec![];
    let mut reply: Option<String> = None;
    for r in &rows {
        match r.st("kind").as_str() {
            "reply" => reply = reply_end(r),
            k if k == MADE => {
                if let (Some(p), Ok(made)) = (prompts.last_mut(), r.st("data").parse()) {
                    p.made.push(made);
                }
            }
            _ => {
                let (text, clipped) = typed(r);
                prompts.push(Prompt { text, made: vec![], clipped, board: r.s("data") == Some(BOARD_PROMPT), reply: reply.take() });
            }
        }
    }
    prompts.reverse();
    Ok(word_in(&prompts, id).map(|i| prompts[i].text.clone()))
}

/// Notes on terminal `sid` that its conversation made goal `id` (`tb goal new`): the goal "run it" and
/// "the goal" can mean.
pub fn note_made_goal(app: &App, sid: &str, id: i64) -> Result<()> {
    crate::board::session_event_with(app, sid, MADE, &format!("Added {}", rf("goal", id)), None, Some(&rf("goal", id)))
}

/// The words that say yes ("yes", "sure", "go ahead", "do it").
const YES: &str = r"(?:yes|yeah|yep|yup|ya|y|k|kk|sure\s+thing|sure|ok(?:ay)?|alright|all\s+right|please\s+do|do\s+it|go\s+ahead|go\s+for\s+it|ship\s+it|let[’']?s\s+(?:do\s+it|go)|absolutely|definitely|of\s+course|for\s+sure|sounds\s+good|lgtm|looks\s+good|yes\s+please|make\s+it\s+so|please|go|👍[\u{1F3FB}-\u{1F3FF}]?)";
/// The yes words that answer on their own, with more said after them ("yes - and keep the PR small"),
/// not ones that may start something else ("go fix the header", "ok, but first …").
const STRONG_YES: &str = r"(?:yes|yeah|yep|yup|sure\s+thing|sure|please\s+do|do\s+it|go\s+ahead|go\s+for\s+it|absolutely|definitely|of\s+course|for\s+sure|sounds\s+good|lgtm|looks\s+good|yes\s+please|make\s+it\s+so|👍[\u{1F3FB}-\u{1F3FF}]?)";
/// Words around a yes that add nothing ("thanks", "great", "now").
const YES_FILL: &str = r"(?:thanks|thank\s+you|ty|great|perfect|cool|nice|awesome|please|pls|now|then|so|go|run\s+it)";
/// A prompt that's only a yes ("yes", "sure, go ahead", "yep, thanks"): the answer to the agent's question.
static SHORT_YES_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(r"(?i)^(?:{YES_FILL}[\s,.!]*)*{YES}(?:[\s,.!]+(?:{YES}|{YES_FILL}))*[\s,.!]*$")).unwrap()
});
/// A prompt that starts with a yes and goes on after a stop or a dash ("Yes. Also rename T9 to Footer.",
/// "yes - and keep the PR small"), or a "but" or an "and" ("yes but do not start T9").
static YES_THEN_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(r"(?is)^(?:{YES_FILL}[\s,.!]*)*{STRONG_YES}(?:[\s,!]+(?:{YES}|{YES_FILL}))*(?P<sep>\s*[.,!;]+|\s*[—–-]+|\s+(?:but|and)\b)\s*(?P<rest>\S.*)$")).unwrap()
});
/// What after a yes makes it no plain yes: an order to it ("yes, but first …", "yes, before that …"). A
/// "but" before a no is read with the no ("yes, but don't start T9").
static YES_BUT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bbut\b(?:\s*,)?\s+(?:(?:don[’']?t|do\s+not|not|never|no|leave|skip|ignore|hold\s+off)\b)|\b(?P<but>but|first|before|instead|rather|or)\b|\?").unwrap());
/// Words that leave the asked one be ("leave it alone", "skip it", "hold off on it").
static LEAVE_IT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:leave|skip|forget|drop|park|hold\s+off\s+on|hold)\s+(?:it|this|that|them|this\s+one|that\s+one)\b").unwrap()
});
/// A prompt that's a yes to the agent's question about starting task (or running goal) `id`: only one,
/// or one with more after it that leaves the start alone. What comes after the yes is read as it would
/// be after the ask the yes stands for ("yes, no rush" as "start T8, no rush", "yes, and ping me when
/// the PR is up" as "start T8, and ping me when the PR is up"): a no, a time or a take-back on the start
/// holds it ("yes, don't start yet", "yes, tomorrow"), one on something else doesn't ("yes. Don't touch
/// the footer though.").
fn says_yes(k: &Kind, prompt: &str, id: i64) -> bool {
    let own = owners_text(prompt);
    let own = own.trim();
    if SHORT_YES_RE.is_match(own) {
        return true;
    }
    let Some(c) = YES_THEN_RE.captures(own) else { return false };
    let rest = c.name("rest").map(|m| m.as_str()).unwrap_or_default();
    // A no after the yes ("yep, nope, hold") or one that leaves the asked one be ("yes but leave it
    // alone") takes it back.
    if STOP_RE.is_match(rest) || STARTS_NO_RE.is_match(rest) || LEAVE_IT_RE.is_match(rest) || YES_BUT_RE.captures_iter(rest).any(|c| c.name("but").is_some() || c.get(0).is_some_and(|m| m.as_str() == "?")) {
        return false;
    }
    let ask = if k.goal { format!("run G{id}") } else { format!("start T{id}") };
    let sep = c.name("sep").map(|m| m.as_str()).unwrap_or(".");
    read_of(k, &format!("{ask}{sep} {rest}")).word_for(id, |_| false) == Some(true)
}
/// A prompt that names only tasks the agent asked to start, as the answer to which ("just T8" to
/// "Should I start T8 and T9?").
static ONLY_NAMED_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(r"(?i)^(?:(?:{STRONG_YES}|ok(?:ay)?|just|only)[\s,.!]*)*{NAMED}(?:[\s,]+(?:only|please|pls|thanks))*[\s.!]*$")).unwrap()
});
/// A prompt that's only an unnamed ask for a goal's run ("run it", "ok, kick it off", "let's run it",
/// "can you run it?", "ship it").
static GOAL_BARE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)^(?:(?:{YES}|{YES_FILL}|let[’']?s|let\s+us|(?:can|could|would|will)\s+(?:you|u)|you\s+can|go\s+ahead\s+and|just)[\s,.!]*)*(?:(?:start|queue|begin|launch|run|ship|get\s+going\s+on|go\s+ahead\s+with)\s+(?:it|this|that|them)|(?:kick\s+(?:it|this|that|them)\s+off|get\s+(?:it|this|that|them)\s+going|(?:spin|fire)\s+(?:it|this|that|them)\s+up))(?:\s+(?:off|up|now|please|pls|right\s+away|right\s+now))*[\s,.!?]*(?:(?:thanks|thank\s+you|ty|please|pls)[\s.!]*)?$"
    ))
    .unwrap()
});
/// A word for a thing that starts, other than a task or a goal: what "start it" can mean instead
/// ("The dev server stopped. Should I start it?", "Wrote scripts/seed.sh.", "I patched the backfill job.").
/// A README, a ticket, a branch, the docs or a PR is no such thing.
static START_THING_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)^(?:scripts?|migrations?|jobs?|servers?|services?|workers?|containers?|docker|emulators?|simulators?|databases?|dbs?|daemons?|",
        r"process(?:es)?|pipelines?|builds?|deploys?|deployments?|crons?|instances?|vms?|clusters?|watchers?|tunnels?|prox(?:y|ies)|backfills?|",
        r"seeds?|seeders?|apps?|clis?|programs?|binar(?:y|ies)|codemods?|storybook|rake|benchmarks?)$"
    ))
    .unwrap()
});
/// A test that's a run of its own, which starts ("I put together a load test. Want me to kick it off?"),
/// not the tests of the work ("I ran the tests").
static OWN_RUN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:load|stress|perf|performance|soak|smoke|e2e|end-to-end|canary)$").unwrap());
static TESTS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:tests?|runs?|suites?)$").unwrap());
/// A word for a thing that runs but doesn't start, what "run it" can mean instead of the goal ("I wrote a
/// quick benchmark.", "The tests for the goal are written.").
static RUN_THING_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)^(?:tests?|specs?|suites?|benchmarks?|bench|linters?|lint|formatters?|quer(?:y|ies)|commands?|notebooks?|experiments?|",
        r"simulations?|playbooks?|makefile|workflows?|smoke|functions?|tools?|installers?|backups?|snippets?)$"
    ))
    .unwrap()
});
/// A file that runs ("seed.sh", "scripts/backfill.py").
static RUN_FILE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^[\w./-]+\.(?:sh|bash|zsh|py|js|mjs|ts|rs|rb|go|pl|php|sql|ps1|bat)$").unwrap());

/// Whether `word` is a thing that starts, or, for a goal's "run it" (`run`), one that runs: a word for one,
/// a file that runs, or a path in a folder for them ("scripts/seed.sh").
fn thing(word: &str, run: bool) -> bool {
    let w = word.trim_end_matches(['.', ',', '!', '?', ':', ';', '\'', '’']);
    let one = |w: &str| START_THING_RE.is_match(w) || (run && RUN_THING_RE.is_match(w));
    RUN_FILE_RE.is_match(&w.replace(FILE_DOT, ".")) || one(w) || (w.contains(['/', '-']) && w.split(['/', '-']).any(one))
}
/// What stands for the dot of a file's name in a sentence of the agent's ("seed.sh"), so it ends no
/// sentence there.
const FILE_DOT: char = '·';
/// A file's name ("seed.sh", "scripts/backfill.py"), whose dot ends no sentence.
static FILE_NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b([\w/-]+)\.(sh|bash|zsh|py|js|mjs|ts|rs|rb|go|pl|php|sql|ps1|bat|toml|json|ya?ml|md|txt)\b").unwrap());
/// Words for the goal or its plan ("the plan is ready", "two tasks", "each task opens its own PR").
static GOAL_NOUN_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:goals?|plans?|tasks|(?:\d+|two|three|four|five|six|seven|eight|nine|ten|each|every|both|all)\s+(?:new\s+)?tasks?)\b").unwrap()
});
/// A line that leads into a summary of the goal ("Here's the plan:", "Here's a summary:").
static SUMMARY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^\s*(?:here[’']?s|here\s+(?:is|are)|below\s+is|this\s+is)\s+(?:the|a|my|our)\s+(?:(?:quick|short|brief|full|final|updated|revised)\s+)?(?:plan|summary|overview|breakdown|outline|rundown|recap|split)\b").unwrap()
});
/// The words of a sentence, for the name of a thing in it.
static WORD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\w./’'·-]+").unwrap());
/// Where a thing's name surely ends: a stop, a comma, a colon, a bracket or a dash.
static NAME_END_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[,;:()\[\]{}—–!?]|\s-+\s").unwrap());
/// A word before a thing's name ("the", "a", "its", "two").
static DET_WORD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(?:the|a|an|my|our|your|their|this|that|these|those|its|some|another|each|every|both|all|one|two|three|four|five|six|\d+)$").unwrap()
});
/// A word no name goes on past: a word that joins, a verb that says something of it, a pronoun.
static NAME_STOP_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)^(?:to|for|of|on|in|at|with|without|that|which|who|and|or|but|so|is|are|was|were|be|been|being|has|have|had|needs?|will|would|",
        r"can|could|should|may|might|must|it|it[’']s|that[’']s|there[’']s|here[’']s|let[’']s|what[’']s|from|into|onto|by|as|then|than|now|also|",
        r"just|there|here|i|we|you|they|he|she|me|us|them|no|nothing|none|not|more|still|only|already|first|next|too|again|ready|done|",
        r"stopped|crashed|died|exited|failed|went|hung|timed|looks?|seems?|passe[sd]|works|worked)$"
    ))
    .unwrap()
});
/// A name's last word that's the goal or its plan ("the plan", "the dark mode goal", "two tasks").
static GOAL_HEAD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:goals?|plans?)$").unwrap());
/// A name's last word that's a task of the goal ("its first task", "a Colors task", "the docs task"), but
/// not with a word before it that makes it a thing of its own ("a rake task", "a cron task").
static TASK_HEAD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^tasks?$").unwrap());
/// What after a thing's name says it's there or not running ("is ready", "stopped", "'s up", "looks
/// good").
static STATE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)^\s*(?:(?:\s(?:is|are|was|were)|[’']s)\s+(?:now\s+|all\s+|also\s+|fully\s+|finally\s+)?",
        r"(?:ready|done|written|built|up|down|running|stopped|set\s+up|in\s+place|finished|complete|available|installed|configured|added|drafted|updated|",
        r"safe|green|passing|tested|fixed|working|good|fine|merged|approved|up\s+to\s+date)|",
        r"\s(?:stopped|crashed|died|exited|failed|went\s+down|hung|timed\s+out|looks?\s+(?:good|fine|great|right|ok(?:ay)?)|passe[sd]|works|worked))\b"
    ))
    .unwrap()
});
/// The agent's own work on a thing ("I wrote seed.sh", "Wrote the goal's migration", "I can run the
/// linter", "Updated the plan", "I fixed the dev server"): the thing is what's after it.
static OWN_WORK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)(?:^\s*|\b(?:i|we)(?:[’']ve|[’']ll|[’']d|\s+have|\s+just|\s+also|\s+can|\s+could|\s+will|\s+went\s+ahead\s+and)*\s+)",
        r"(?:wrote|rewrote|built|made|created|added|drafted|set\s+up|installed|configured|generated|scaffolded|put\s+together|spun\s+up|whipped\s+up|",
        r"updated|prepared|run|ran|started|launched|fixed|patched|tested|checked|reviewed|verified|validated|refactored|tweaked|changed|",
        r"edited|cleaned\s+up|debugged|rebased|wired\s+up|hooked\s+up|improved)\s+"
    ))
    .unwrap()
});
/// A sentence that brings in a thing the work needs or has ("Its first task needs a seed script", "In
/// the goal there's a codemod").
static NEEDS_A_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:needs?|requires?|there[’']?s|there\s+(?:is|are)|is\s+missing|calls\s+for)\s+(?:a|an|one|another|some|the)\s").unwrap()
});
/// A sentence that hands over a thing ("Here's the goal's migration", "Attached is the seed script").
static PRESENT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^\s*|\b)(?:here[’']?s|here\s+(?:is|are)|attached\s+(?:is|are)|below\s+(?:is|are)|i[’']ve\s+attached)\s+").unwrap());
/// What may follow a thing's name and only say which it is ("the migration for the goal", "the PR for the
/// footer", "the tests for G4").
static WHICH_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^\s+(?:for|of|in|on|from|behind)\s+(?:(?:the|this|that|our|its|your|my|a|an)\s+)?[\w.-]+(?:\s+(?:goal|plan|task|screen|page|work))?").unwrap()
});
/// A word before "task" that makes it a thing of its own ("a rake task", "a cron task").
static RUNNER_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^(?:rake|cron|gradle|npm|yarn|make|grunt|gulp|celery|background|scheduled|async|airflow|sidekiq|resque|gulp)$").unwrap());
/// A line of a list ("- Colors: pick them", "1. Toggle"), which goes with the line it follows.
static LIST_LINE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*(?:[-*•]|\d{1,3}[.)])\s").unwrap());
/// An aside in brackets ("(T9 covers the rest)"), which is no sentence's subject.
static ASIDE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\([^()\n]*\)?|\[[^\[\]\n]*\]?").unwrap());
/// The owner asks the agent to make something ("set up the emulator", "write a backfill"): the thing is
/// what's after it.
static MAKE_THING_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:make|create|add|write|build|set\s+up|draft|generate|scaffold|install|configure|put\s+together|spin\s+up|whip\s+up|plan)\s+").unwrap()
});
/// The owner asks for something as a goal ("plan the login tests as a goal").
static AS_GOAL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:as|into)\s+(?:a|one|the)\s+(?:new\s+|single\s+)?goal\b").unwrap());
/// The agent made the tasks or goals its sentence names ("I made T10 and T11", "Added T10: …", "Made
/// G4 with two tasks").
static MADE_REFS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:made|added|created|filed|opened|drafted|logged|set\s+up|proposed|planned|put|split)\b[^.!?;\n]*?\b[TtGg]\d+\b").unwrap()
});
/// The refs in a row that a made word is on ("T10 and T11", "T10, T11 & T12").
static MADE_LIST_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^[TtGg]\d+(?:(?:\s*,\s*(?:and\s+)?|\s+and\s+|\s*&\s*|\s*\+\s*)[TtGg]\d+)*").unwrap());

/// What a sentence speaks of, which an "it" after it can mean: the goal or its plan, or something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Topic {
    Goal,
    Other,
}

/// The thing named at the start of `s` ("the dev database", "a goal-level smoke test", "the goal's
/// migration", "the dark mode goal"), what it is, and where its name ends. The thing is the name's last
/// word: a word before it only says which ("the plan's seed script" is a seed script, not the plan). It's
/// the goal or its plan, or something else only when it starts (or, for a goal's run, `run`, runs): a
/// README, a ticket, a branch or the docs is `None`. "It", "this" and "nothing" name nothing.
fn name_at(s: &str, run: bool) -> Option<(Option<Topic>, usize)> {
    let s = &s[..NAME_END_RE.find(s).map_or(s.len(), |m| m.start())];
    let words: Vec<regex::Match> = WORD_RE.find_iter(s).collect();
    let mut name: Vec<&str> = vec![];
    let mut end = 0;
    for (i, w) in words.iter().enumerate() {
        let word = w.as_str();
        if NAME_STOP_RE.is_match(word) {
            break;
        }
        if name.is_empty() && DET_WORD_RE.is_match(word) {
            continue;
        }
        if let Some(stem) = word.strip_suffix("'s").or_else(|| word.strip_suffix("’s")) {
            // "The emulator's ready": an "is", not whose.
            let next = words.get(i + 1).map(|n| n.as_str());
            if next.is_none_or(|n| NAME_STOP_RE.is_match(n) || STATE_RE.is_match(&format!(" is {n}"))) {
                name.push(stem);
                end = w.start() + stem.len();
                break;
            }
            // Whose: what follows is the thing.
            name.clear();
            continue;
        }
        name.push(word);
        end = w.end();
        if name.len() == 5 {
            break;
        }
    }
    let (head, mods) = name.split_last()?;
    let topic = if GOAL_HEAD_RE.is_match(head) {
        Some(Topic::Goal)
    } else if TASK_HEAD_RE.is_match(head) {
        Some(if mods.iter().any(|m| RUNNER_RE.is_match(m) || thing(m, run)) { Topic::Other } else { Topic::Goal })
    } else {
        let own_run = TESTS_RE.is_match(head) && mods.iter().any(|m| OWN_RUN_RE.is_match(m));
        (own_run || name.iter().any(|w| thing(w, run))).then_some(Topic::Other)
    };
    Some((topic, end))
}

/// What sentence `s` speaks of, if it speaks of anything an "it" after it can mean: a thing as the
/// agent's own work ("I wrote seed.sh", "Updated the plan", "I can run the linter on the plan files"),
/// as there or not running ("The emulator is ready", "The docker container stopped", "The plan's seed
/// script is ready"), or as what the work needs ("Its first task needs a seed script"); or else the goal
/// or its plan when it names them ("The goal has two tasks: …", "Each task opens its own PR"). Something
/// else wins over the goal ("For the goal I wrote seed.sh"). A sentence that only says something of what
/// was named before ("It touches one file", "The changes are small", "My guess is a day of work") speaks
/// of nothing new.
#[cfg(test)]
fn topic_of(s: &str) -> Option<Topic> {
    topic_in(s, true)
}

/// [`topic_of`], for a start (`run` false: a test or a linter isn't what "start it" means) or a goal's run.
/// "The goal's X" and "X for the goal" speak of X.
fn topic_in(s: &str, run: bool) -> Option<Topic> {
    let s = ASIDE_RE.replace_all(s, " ");
    let mut seen: Vec<Topic> = vec![];
    for m in OWN_WORK_RE.find_iter(&s).chain(PRESENT_RE.find_iter(&s)) {
        seen.extend(name_at(&s[m.end()..], run).and_then(|(t, _)| t));
    }
    for (a, b) in clauses(&s) {
        let c = &s[a..b];
        let lead = c.len() - c.trim_start().len();
        let c = JOIN_LEAD_RE.replace(&c[lead..], "");
        if let Some((t, end)) = name_at(&c, run) {
            let rest = &c[end..];
            let rest = WHICH_RE.find(rest).map_or(rest, |m| &rest[m.end()..]);
            if STATE_RE.is_match(rest) {
                seen.extend(t);
            }
        }
    }
    for m in NEEDS_A_RE.find_iter(&s) {
        if let Some((Some(Topic::Other), _)) = name_at(&s[m.end()..], run) {
            seen.push(Topic::Other);
        }
    }
    if seen.contains(&Topic::Other) {
        return Some(Topic::Other);
    }
    (!seen.is_empty() || GOAL_NOUN_RE.is_match(&s) || SUMMARY_RE.is_match(&s)).then_some(Topic::Goal)
}
/// A joining word a clause starts with ("and the emulator is ready").
static JOIN_LEAD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:(?:and|so|but|then|also|plus|ok(?:ay)?|now)\b[\s,]*)+").unwrap());

/// What the agent's message before its question speaks of last, which an "it" or "them" in the question
/// means.
enum Antecedent {
    Refs(Refs),
    Topic(Topic),
}
/// Tasks and goals named, in order: (a goal, its number).
type Refs = Vec<(bool, i64)>;

/// One sentence of the agent's message, for [`antecedent`]: what it names (less what's in brackets: "I
/// made T10 and T11 (T9 covers the rest)" names T10 and T11), what a list it leads into names ("Made G4:\n-
/// T10 Colors\n- T11 Toggle"), what it speaks of ([`topic_of`]), and whether it says the agent made what
/// it names ("I made T10", "Added T10: …").
struct Said {
    refs: Refs,
    list: Refs,
    topic: Option<Topic>,
    made: bool,
    /// The tasks or goals it says the agent made: the ones right after the word ("I made T10 because T8
    /// got too big" made T10).
    made_refs: Refs,
}

impl Said {
    /// What it names: its own refs, or its list's when it names none ("I made two tasks:\n- T10 …").
    fn named(&self) -> &Refs {
        if self.refs.is_empty() {
            &self.list
        } else {
            &self.refs
        }
    }
}

/// A clause that says why or since when ("now that T9 merged", "since T9 landed"), whose refs aren't
/// what the sentence speaks of when it names another.
static WHY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:now\s+that|since|because|given\s+that)\b[^,;:]*").unwrap());

fn said_in(text: &str, run: bool) -> Vec<Said> {
    let text = FILE_NAME_RE.replace_all(text, format!("${{1}}{FILE_DOT}${{2}}").as_str());
    let refs = |t: &str| -> Refs {
        let t = ASIDE_RE.replace_all(t, " ");
        let main = WHY_RE.replace_all(&t, " ");
        let main = refs_in(&main);
        if main.is_empty() {
            refs_in(&t)
        } else {
            main
        }
    };
    let made_refs = |t: &str| -> Refs {
        let t = ASIDE_RE.replace_all(t, " ");
        let Some(m) = MADE_REFS_RE.find(&t) else { return vec![] };
        let first = REF_RE.find_iter(m.as_str()).last().map_or(m.end(), |r| m.start() + r.start());
        MADE_LIST_RE.find(&t[first..]).map(|l| refs_in(l.as_str())).unwrap_or_default()
    };
    let mut said: Vec<Said> = vec![];
    // The list's lines go with the line before them when that line leads into it ("Made two tasks:").
    let mut lead = false;
    let mut in_list = false;
    for line in text.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        if LIST_LINE_RE.is_match(line) {
            match said.last_mut() {
                Some(s) if lead || in_list => s.list.extend(refs(line)),
                _ => said.push(Said { refs: vec![], list: refs(line), topic: None, made: false, made_refs: vec![] }),
            }
            in_list = true;
            continue;
        }
        lead = line.trim_end().ends_with(':');
        in_list = false;
        for c in SENTENCE_RE.captures_iter(line) {
            let Some(s) = c.name("s").filter(|s| !s.as_str().trim().is_empty()) else { continue };
            let s = s.as_str();
            let made = MADE_REFS_RE.is_match(&ASIDE_RE.replace_all(s, " ")) || (lead && OWN_WORK_RE.is_match(s));
            said.push(Said { refs: refs(s), list: vec![], topic: topic_in(s, run), made, made_refs: made_refs(s) });
        }
    }
    said
}

/// What "it" or "them" in the agent's question means, from the message before it, `text`.
///
/// For a goal (`goal`), the nearest sentence that names or speaks of something: the goal it names ("Made
/// G4. The work is mostly UI."), or what it speaks of ([`topic_of`]); a sentence that only says more of
/// what came before ("This touches the header") is passed over. The goal the agent says it made is what
/// it made, not the tasks it made it with ("Made G4 with T10 and T11").
///
/// For tasks, the tasks the agent says it made, the latest that it does ("I made T10. T8 is still in
/// review."), or else the nearest sentence that names tasks ("T8 is ready. It only touches the header."),
/// unless a sentence after it brings in something else that could be started ("T8 is ready. The dev
/// server stopped.").
fn antecedent(text: &str, goal: bool) -> Option<Antecedent> {
    let said = said_in(text, goal);
    let made_goal = |s: &Said| s.made && s.named().first().is_some_and(|r| r.0);
    if goal {
        return said.iter().rev().find_map(|s| {
            if made_goal(s) {
                Some(Antecedent::Refs(s.named().iter().copied().filter(|r| r.0).collect()))
            } else if s.topic == Some(Topic::Other) {
                // "I wrote the migration for G4": the migration.
                Some(Antecedent::Topic(Topic::Other))
            } else if !s.named().is_empty() {
                Some(Antecedent::Refs(s.named().clone()))
            } else {
                s.topic.map(Antecedent::Topic)
            }
        });
    }
    let tasks = |s: &Said| -> Refs {
        // "Made G4 with T10 and T11", "Made G4:\n- T10 …": the tasks it made.
        let all: Refs = s.refs.iter().chain(&s.list).copied().collect();
        if made_goal(s) {
            all.into_iter().filter(|r| !r.0).collect()
        } else if s.made && !s.made_refs.is_empty() {
            s.made_refs.clone()
        } else {
            s.named().clone()
        }
    };
    if let Some(s) = said.iter().rev().find(|s| s.made && !tasks(s).is_empty()) {
        return Some(Antecedent::Refs(tasks(s)));
    }
    let at = said.iter().rposition(|s| !s.named().is_empty())?;
    // "T8 needs the worker. I've scaffolded it.": the worker.
    if said[at..].iter().any(|s| s.topic == Some(Topic::Other)) {
        return None;
    }
    Some(Antecedent::Refs(said[at].named().clone()))
}

/// What the owner's prompt asks the agent to make, which an "it" after it can mean ("set up the
/// emulator", "make a goal for adding tests", "write the goal's migration"), or, when it asks to make
/// nothing, something else when it names a script, a test or the like ("run the linter").
fn prompt_topic(owners: &str) -> Option<Topic> {
    if AS_GOAL_RE.is_match(owners) {
        return Some(Topic::Goal);
    }
    // "Make the goal's PRs smaller", "add acceptance criteria to each task": what it changes or adds
    // starts nothing.
    if MAKE_THING_RE.is_match(owners) {
        return MAKE_THING_RE.find_iter(owners).find_map(|m| name_at(&owners[m.end()..], true).and_then(|(t, _)| t));
    }
    WORD_RE.find_iter(owners).any(|w| thing(w.as_str(), true)).then_some(Topic::Other)
}
/// The prompt asks for a new goal ("make a goal for dark mode").
static MAKE_GOAL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:make|create|add|set\s+up|draft|write\s+up|put\s+together|plan\s+out|a\s+new)\b[^.!?;\n]*\bgoals?\b(?:[^-]|$)|",
        r"\b(?:plan|make|turn|put)\b[^.!?;\n]*\b(?:as|into)\s+(?:a|one|the)\s+(?:new\s+|single\s+)?goal\b"
    ))
    .unwrap()
});
/// Words in the agent's question that make "yes" no answer to one thing ("run it, or tweak the plan?").
static OR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:or|either|instead|rather)\b").unwrap());
/// A task or goal named.
static REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b([TtGg])(\d+)\b").unwrap());

/// Whether the agent's message `reply` ends on asking whether to run goal `id` now: its last question,
/// with only statements after it, has one start word for the goal in it, named ("Want me to run G4?"),
/// as "the goal" when it's the conversation's (`ours`), or as "it" when the message names G4 before it
/// and no other goal or task ("I added T1 to G4. Want me to start it?" may mean T1), or names nothing,
/// G4 is the conversation's one goal (`only`) and what the message speaks of just before it is the goal
/// or its plan, or nothing ("The plan is ready. Want me to run it?", "Each task opens its own PR. Want
/// me to run it?", but not "Wrote seed.sh. Want me to run it?" or "I set up the dev database. Should I
/// start it?"). A no, a time, a condition or a choice in the question ("or tweak it first?") makes a
/// "yes" no word.
pub fn asks_to_run(reply: &str, id: i64, ours: bool, only: bool) -> bool {
    asked_in(&GOALS, reply, id, ours, only, None).contains(&id)
}

/// Whether the agent's message `reply` ends on asking whether to start task `id` now ("Should I start
/// T8?", "Want me to kick off T8 and T9 now?"), read as [`asks_to_run`] reads a goal's: by name, or as
/// "it" when the message names T8 before it and no other task or goal, or as "them" for the tasks it
/// names before it.
pub fn asks_to_start(reply: &str, id: i64) -> bool {
    asked_tasks(reply, id).contains(&id)
}

/// The tasks the agent's message `reply` ends on asking whether to start ([`asks_to_start`]).
fn asked_tasks(reply: &str, id: i64) -> Vec<i64> {
    asked_in(&TASKS, reply, id, false, false, None)
}

/// An offer that's a question ("I can start T8 now if you want.", "Ready to start T8 whenever you
/// are."): read as "Want me to start T8 now?".
static OFFER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?im)(?P<lead>^|[.!?]\s+)[\s(]*(?:i\s+can|i\s+could|i[’']?m\s+(?:ready|happy|able)\s+to|ready\s+to|happy\s+to)\s+(?P<what>[^.!?\n]*?)\s*,?\s*",
        r"(?:if\s+you(?:[’']d)?\s+(?:want|like)(?:\s+me\s+to)?|whenever\s+you(?:[’']re|\s+are)(?:\s+ready)?|when\s+you(?:[’']re|\s+are)\s+ready|if\s+that\s+works(?:\s+for\s+you)?|just\s+say\s+the\s+word|on\s+your\s+(?:go|word|say))\s*[.!]+"
    ))
    .unwrap()
});
/// A question to the owner about what the agent would do ("Should I …", "Want me to …"), which may have
/// statements after it ("Want me to start T8? It'll take a while.").
static ADDRESSED_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s(]*(?:(?:so|ok(?:ay)?|alright|and|also|then|now)[\s,]+)*(?:(?:should|shall|can|could|may)\s+i|(?:do\s+you\s+)?want\s+me\s+to|would\s+you\s+like\s+(?:me\s+)?to|(?:is\s+it\s+)?ok(?:ay)?\s+(?:if\s+i|to))\b").unwrap()
});
/// A question that leaves out what it's about, which the sentence before it says ("I can start T8
/// next. Want me to?").
static ELLIPTIC_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\s(]*(?:(?:so|ok(?:ay)?|alright|and|also|then|now)[\s,]+)*(?:(?:should|shall)\s+i|(?:do\s+you\s+)?want\s+me\s+to|would\s+you\s+like\s+me\s+to)(?:\s+(?:go\s+ahead|do\s+(?:it|that|so)|proceed))?\s*$").unwrap()
});

/// What [`asks_to_run`] and [`asks_to_start`] read: the tasks or goals the agent's question asks to
/// start. `id` is the one asked about, which "it" or "the goal" can mean: `ours` says "the goal" is it,
/// `only` says "it" is when the message speaks of the goal or its plan just before it, or of nothing (it's
/// the conversation's one goal). `prompt` is what the owner's prompt before the message asked the agent
/// to make ([`prompt_topic`]), which "it" means when the message speaks of nothing ("Done. Want me to
/// run it?").
fn asked_in(k: &Kind, reply: &str, id: i64, ours: bool, only: bool, prompt: Option<Topic>) -> Vec<i64> {
    let reply = CODE_RE.replace_all(reply, "\n");
    let reply = OFFER_RE.replace_all(&reply, "${lead}Want me to ${what}?");
    // (where, sentence, a question)
    let ss: Vec<(usize, &str, bool)> = SENTENCE_RE
        .captures_iter(&reply)
        .filter_map(|c| {
            let s = c.name("s").filter(|s| !s.as_str().trim().is_empty())?;
            Some((s.start(), s.as_str(), c.name("end").is_some_and(|e| e.as_str().contains('?'))))
        })
        .collect();
    // The last question; any sentence after it a statement that puts nothing off and offers no choice.
    let Some(qi) = ss.iter().rposition(|s| s.2) else { return vec![] };
    let (mut at, mut q, _) = ss[qi];
    let after = &ss[qi + 1..];
    // What the question is about: what it names, or else what the message names before it.
    let own = match refs_in(q) {
        r if r.is_empty() => refs_in(&reply[..at]),
        r => r,
    };
    if !after.is_empty() && (!ADDRESSED_RE.is_match(q) || after.iter().any(|(_, s, _)| holds_off(k, s, &own))) {
        return vec![];
    }
    // "Want me to?": the start is in the sentence before.
    if ELLIPTIC_RE.is_match(q) && qi > 0 && !ss[qi - 1].2 {
        (at, q, _) = ss[qi - 1];
    }
    if !defers_in(q).is_empty() || OR_RE.is_match(q) {
        return vec![];
    }
    // Not a start on something of a task's ("Want me to run T8's tests?", "Should I start T8's review?"),
    // and not "T8 now" or "T8 next" with some other verb on it ("Should I close T8 now?"): in a question
    // only a start word asks to start.
    let ms: Vec<Mention> =
        k.mention.captures_iter(q).filter(|c| c.name("named5").is_none()).filter_map(|c| mention_of(k, &c)).filter(|m| !of_a_tasks(q, m)).collect();
    let [m] = ms.as_slice() else { return vec![] };
    if !negs_in(&q[..m.verb]).is_empty() {
        return vec![];
    }
    let pronoun = PRONOUN_RE.is_match(&q[m.span.0..m.span.1]);
    let unnamed = matches!(m.ask, Ask::It) || (!k.goal && (pronoun || m.ask == Ask::Them));
    if !unnamed {
        return match &m.ask {
            Ask::Named(ids) => ids.clone(),
            _ if ours && k.goal => vec![id],
            _ => vec![],
        };
    }
    // What "it" or "them" means: what the message speaks of just before it.
    let mut refs: Vec<(bool, i64)> = vec![];
    match antecedent(&reply[..at + m.verb], k.goal) {
        Some(Antecedent::Refs(named)) => {
            for r in named {
                if !refs.contains(&r) {
                    refs.push(r);
                }
            }
        }
        Some(Antecedent::Topic(Topic::Goal)) => return if k.goal && only { vec![id] } else { vec![] },
        Some(Antecedent::Topic(Topic::Other)) => return vec![],
        // "Should I start it? T8 is ready.": the task the statement after it speaks of.
        None if !k.goal && m.ask != Ask::Them => {
            return after.first().and_then(|(_, s, _)| READY_REF_RE.captures(s)).and_then(|c| c[1].parse().ok()).into_iter().collect();
        }
        None => return if k.goal && only && prompt != Some(Topic::Other) { vec![id] } else { vec![] },
    }
    match (&m.ask, refs.as_slice()) {
        // "Want me to start them?" after the tasks it means.
        (Ask::Them, _) if refs.iter().all(|r| !r.0) => refs.iter().map(|r| r.1).collect(),
        (Ask::Them, _) => vec![],
        (_, [(goal, n)]) if *goal == k.goal => vec![*n],
        _ => vec![],
    }
}

/// A caveat on the start in a statement after the agent's question ("T9 needs to merge first though").
static CAVEAT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:though|tho|however|prerequisite)\b").unwrap());
/// A word that puts something before the start ("T9 goes first", "T9 has to land before it")…
static ORDER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:first|before)\b").unwrap());
/// …and one that says something has to happen ("First, the API change needs to merge").
static NEED_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:needs?|needed|has\s+to|have\s+to|must|should|ought|got\s+to|gotta|requires?|required|let\s+me|want\s+to|wait)\b").unwrap()
});
/// A word that says the statement's subject waits or is blocked ("It waits on T9", "I'd hold off until T9
/// merges", "It depends on T9"): a hold on the start unless the subject is another task ("T9 can wait",
/// "T9 is blocked on it")…
static WAITS_ON_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:blocked|depends|depending|waits|wait|waiting|hold\s+(?:off|on|until|till|back))\b").unwrap());
/// …and one that says something is in its way, whatever the subject ("T9 blocks it", "T9 has a blocker
/// for it").
static BLOCKS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:blocks|blocking|blocker|blockers|dependency|dependencies)\b").unwrap());
/// The task or goal a statement starts with as its subject ("T9 can wait", "T8 is ready").
static SUBJECT_REF_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s*(?:(?:so|and|but|ok(?:ay)?|also|now|well)[\s,]+)*(?:task\s+|goal\s+)?([TtGg])(\d+)\b").unwrap());
/// A statement whose subject is something else, not a task or the goal ("Each PR waits for your review").
static THING_SUBJECT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s*(?:(?:so|and|but|ok(?:ay)?|also|now|well)[\s,]+)*(?:the|each|every|a|an|my|your|our|its)\s+([\w-]+)").unwrap());
/// What such a subject is when it's the start's own ("the goal", "each task", "the work").
static OWN_THING_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:goals?|plans?|tasks?|work|start|run|change|first)$").unwrap());
/// A statement whose subject is the start's own ("It should go first", "This is first in line").
static IT_SUBJECT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^\s*(?:(?:so|and|but|ok(?:ay)?|also|now|well)[\s,]+)*(?:it|this|that)\b").unwrap());
/// A statement that speaks of one task as ready or done, as its subject ("T8 is ready", "T8 looks good").
static READY_REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^\s*(?:task\s+)?[Tt](\d+)\s*(?:is|[’']s|looks|seems|has\s+been)\b").unwrap());
/// A condition that only follows on from the start ("when it's done", "once it lands").
static FOLLOWS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:when|whenever|once|after)$").unwrap());
/// "Until", which follows on from the start when it's on the agent's own later work ("I'll hold the PR
/// as a draft until you look"), but not when the agent waits ("I'd wait until T9 merges").
static UNTIL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:until|till|til)$").unwrap());
static WAITS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:wait|hold\s+(?:off|on|back|it|this|that|them)|pause|hang\s+on)\b").unwrap());
/// Words that point at the start ("do it tomorrow", "start it later").
static POINTS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:do|start|run|begin|queue|launch|kick\s+off|pick\s+up)\s+(?:it|that|this|them)\b").unwrap());

/// The tasks and goals `s` names, as (a goal, its number).
fn refs_in(s: &str) -> Refs {
    REF_RE.captures_iter(s).filter_map(|r| Some((r[1].eq_ignore_ascii_case("g"), r[2].parse().ok()?))).collect()
}

/// Whether statement `s`, after the agent's question about starting `own` (what it names, or what the
/// message names before it), holds the start off. A hold word holds it unless it's said not to ("Nothing
/// blocks it now", "No need to wait for T9 first") or is about another subject ("T9 can wait", "T9 is
/// blocked on it"). It holds when it offers a choice, puts something else before the start ("T9 goes
/// first", "Let's do T9 first", "T9 has to land before it") or says something has to happen first ("First,
/// the API change needs to merge"), says the start waits or is blocked ("It waits on T9", "T9 blocks it",
/// "I'd hold until the migration merges"), or puts it off ("I could also do it tomorrow"). An order of the
/// start's own work ("It's the first of the two", "It should go first", "I'll write the tests first", "The
/// tests run before each commit") and a condition on what the agent does once it has started ("I'll open
/// a draft PR when it's done", "I'll hold the PR as a draft until you look") hold nothing off.
fn holds_off(k: &Kind, s: &str, own: &[(bool, i64)]) -> bool {
    if OR_RE.is_match(s) || CAVEAT_RE.is_match(s) {
        return true;
    }
    let denied = |at: usize| !negs_in(&s[..at]).is_empty();
    let other = refs_in(s).iter().any(|r| !own.contains(r));
    let subject = SUBJECT_REF_RE.captures(s).and_then(|c| Some((c[1].eq_ignore_ascii_case("g"), c[2].parse::<i64>().ok()?)));
    let other_subject =
        subject.is_some_and(|r| !own.contains(&r)) || THING_SUBJECT_RE.captures(s).is_some_and(|c| !OWN_THING_RE.is_match(&c[1]));
    let own_subject = IT_SUBJECT_RE.is_match(s) || subject.is_some_and(|r| own.contains(&r));
    if ORDER_RE.find_iter(s).any(|m| !denied(m.start()) && (other || (NEED_RE.is_match(s) && !own_subject))) {
        return true;
    }
    if BLOCKS_RE.find_iter(s).any(|m| !denied(m.start())) || WAITS_ON_RE.find_iter(s).any(|m| !denied(m.start()) && !other_subject) {
        return true;
    }
    defers_in(s).iter().any(|m| {
        let lead = &s[..m.start()];
        // "Before" is an order, read above.
        if ORDER_RE.is_match(m.as_str()) {
            return false;
        }
        if WAITS_ON_RE.is_match(m.as_str()) {
            return !denied(m.start()) && !other_subject;
        }
        if UNTIL_RE.is_match(m.as_str()) {
            // "It doesn't have to wait until T9 merges": the wait it's on is said not to be.
            if let Some(w) = WAITS_RE.find_iter(lead).last() {
                return !denied(w.start()) && !other_subject;
            }
        }
        let follows = FOLLOWS_RE.is_match(m.as_str()) || UNTIL_RE.is_match(m.as_str());
        !(follows && SUBJECT_RE.is_match(lead) && !POINTS_RE.is_match(lead) && !k.mention.is_match(lead))
    })
}

/// One prompt of the conversation, for [`goal_word_in`]: the prompt, and what an unnamed ask in it
/// means.
#[derive(Debug, Clone, Default)]
pub struct GoalPrompt {
    pub prompt: Prompt,
    /// "The goal" or "all the tasks" in it means this goal: it's the conversation's goal.
    pub ours: bool,
    /// "Run it" in it means this goal (only the latest prompt's counts): the agent just asked whether to
    /// run it, the prompt asked for a goal and the conversation made this one in reply, or the prompt is
    /// only the ask and the prompt before asked for the goal the conversation then made, or it's the
    /// conversation's one goal.
    pub it: bool,
    /// The agent's message just before it asks whether to run this goal ([`asks_to_run`]): a bare "yes"
    /// in it is the word.
    pub asked: bool,
}

/// A prompt that's only the word to go ("do it", "go for it", "sounds good, go", "make it so"): with
/// "it" the goal ([`GoalPrompt::it`]), the word to run it.
static GOAL_GO_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)^(?:(?:{YES_FILL}|ok(?:ay)?|alright|sounds\s+good|looks\s+good|lgtm|yes|yeah|yep|sure)[\s,.!]*)*(?:do\s+it|go\s+for\s+it|go(?:\s+ahead)?|let[’']?s\s+(?:do\s+it|go)|make\s+it\s+so|ship\s+it)(?:[\s,.!]+(?:now|please|pls|thanks|then))*[\s,.!]*$"
    ))
    .unwrap()
});

/// Which prompt, newest first, is the word to run goal `id`. Read like [`word_in`]: the latest prompt
/// that speaks of the run decides, and a prompt whose end never reached the board is no word. A prompt is
/// also the word when it's only a yes to the agent's question about running it, and it's the latest
/// prompt or only thanks came after it (as far back as an unnamed ask reaches, [`UNNAMED_REACH`]), or
/// when it's the latest and only the word to go ("do it") with "it" the goal.
pub fn goal_word_in(prompts: &[GoalPrompt], id: i64) -> Option<usize> {
    let quiet = |q: &Prompt| !q.boards() && !q.clipped && ACK_RE.is_match(owners_text(&q.text).trim());
    for (i, g) in prompts.iter().enumerate() {
        let p = &g.prompt;
        if p.boards() {
            continue;
        }
        let r = read_of(&GOALS, &p.text);
        let near = i == 0 || (i <= UNNAMED_REACH && prompts[..i].iter().all(|q| quiet(&q.prompt)));
        match r.word_for(id, |a| if *a == Ask::It { i == 0 && g.it } else { g.ours }) {
            Some(true) if p.clipped => {}
            Some(true) => return Some(i),
            Some(false) => return None,
            // A yes may hold another goal off as it goes ("yes but leave G1 alone").
            None if g.asked && near && !p.clipped && only_others(&r) && says_yes(&GOALS, &p.text, id) => return Some(i),
            None if i == 0 && g.it && !p.clipped && r.steps.is_empty() && GOAL_GO_RE.is_match(owners_text(&p.text).trim()) => return Some(0),
            // "now" to "Should I run G2 now or after the release?".
            None if i == 0 && g.asked && !p.clipped && r.steps.is_empty() && NOW_ONLY_RE.is_match(owners_text(&p.text).trim()) => return Some(0),
            None => {}
        }
    }
    None
}

/// Notes on terminal `sid` that goal `id` ran on the owner's word there: the word is spent, so the next
/// run needs a new one.
pub fn note_ran_goal(app: &App, sid: &str, id: i64) -> Result<()> {
    crate::board::session_event_with(app, sid, RAN, &format!("Ran {} on your word", rf("goal", id)), None, Some(&rf("goal", id)))
}

/// Where terminal `sid`'s conversation began: its latest start, or, when that's a `/clear` with nothing
/// typed since, the start before it (the conversation goes on). A new or resumed session with nothing
/// typed yet is a new conversation.
fn conversation_start(app: &App, sid: &str) -> Result<i64> {
    let Some(last) = app.db.q1("SELECT id, text FROM session_events WHERE session_id = ? AND kind = 'start' ORDER BY id DESC LIMIT 1", p![sid])? else {
        return Ok(0);
    };
    let typed = app.db.q1("SELECT COALESCE(MAX(id), 0) AS n FROM session_events WHERE session_id = ? AND kind = 'prompt'", p![sid])?.and_then(|r| r.i("n")).unwrap_or(0);
    if last.id() < typed || last.st("text") != CLEARED {
        return Ok(last.id());
    }
    let before = app.db.q1("SELECT COALESCE(MAX(id), 0) AS n FROM session_events WHERE session_id = ? AND kind = 'start' AND id < ?", p![sid, typed])?;
    Ok(before.and_then(|r| r.i("n")).unwrap_or(0))
}

/// The prompt a human typed in terminal `sid`, since its Claude conversation began, that asks for goal
/// `id` to run, if there's one and no later prompt takes it back. "Run the goal" means `id` when it's the
/// conversation's goal: the terminal's task is in it, the conversation made it or a task in it (`tb goal
/// new`, `tb propose`, `tb task new --goal`), the board opened the terminal to plan it, or (for the
/// prompts after it) the board handed it the goal. A goal that isn't the conversation's runs from it only
/// by name: a prompt that names it ("run G2", "G2 looks good. run it"), or a yes to the agent's question
/// that names it ("G2 has one planned task. Want me to run it?"). "Run it" and a bare "yes" are read as
/// [`GoalPrompt`] says; when the conversation has only this goal of its own, or this is the latest it
/// made, a prompt that's only "run it", and a yes to the agent's "Want me to run it?", mean it. A word is
/// good for one run: once the goal ran on it ([`RAN`]), the next run needs a new one. A conversation
/// cleared with nothing typed since is still the one before ([`conversation_start`]).
pub fn owners_goal_word(app: &App, sid: &str, id: i64) -> Result<Option<String>> {
    if sid.is_empty() {
        return Ok(None);
    }
    let since = conversation_start(app, sid)?;
    // Every prompt, every task and goal the conversation made, every message the agent ended a turn on
    // and every run on a word here, oldest first.
    let rows = app.db.q(
        "SELECT id, kind, text, full, data FROM session_events WHERE session_id = ?1 AND kind IN ('prompt', 'reply', ?2, ?3) AND id > ?4 ORDER BY id",
        p![sid, MADE, RAN, since],
    )?;
    // The conversation's goals: its task's, the ones it made or made a task in, and the ones the board
    // opened it to plan ("Plan in Claude": its system prompt is the goal).
    let mut theirs: std::collections::BTreeSet<i64> = crate::board::task_for_session(app, Some(sid))?.and_then(|t| t.i("goal_id")).into_iter().collect();
    for r in app.db.q(
        "SELECT COALESCE(t.goal_id, CASE WHEN e.data LIKE 'G%' THEN CAST(substr(e.data, 2) AS INTEGER) END) AS goal
           FROM session_events e LEFT JOIN tasks t ON t.id = CAST(e.data AS INTEGER)
          WHERE e.session_id = ?1 AND e.kind = ?2 AND e.id > ?3",
        p![sid, MADE, since],
    )? {
        theirs.extend(r.i("goal"));
    }
    for r in app.db.q(
        "SELECT json_extract(target, '$.goal') AS goal FROM jobs WHERE kind = 'agent' AND purpose = 'plan' AND json_extract(target, '$.session') = ?",
        p![sid],
    )? {
        theirs.extend(r.i("goal"));
    }
    let ours = theirs.contains(&id);
    // Oldest first: (prompt, goals made in reply to it, the agent's message before it).
    let mut said: Vec<(Prompt, Vec<i64>, Option<String>)> = vec![];
    let mut reply: Option<String> = None;
    // The goals the conversation made, oldest first.
    let mut made_goals: Vec<i64> = vec![];
    // How many prompts came before the goal's latest run on a word here: their words are spent.
    let mut spent = 0;
    for r in &rows {
        match r.st("kind").as_str() {
            "reply" => reply = reply_end(r),
            k if k == RAN => {
                if r.st("data") == rf("goal", id) {
                    spent = said.len();
                }
            }
            k if k == MADE => {
                let made: Option<i64> = r.st("data").strip_prefix(['G', 'g']).and_then(|n| n.parse().ok());
                made_goals.extend(made);
                if let (Some(p), Some(g)) = (said.last_mut(), made) {
                    p.1.push(g);
                }
            }
            _ => {
                let (text, clipped) = typed(r);
                said.push((Prompt { text, made: vec![], clipped, board: r.s("data") == Some(BOARD_PROMPT), reply: None }, vec![], reply.take()));
            }
        }
    }
    said.reverse();
    // The prompts after the board's handoff, if any, come before it here.
    let handed = said.iter().position(|(p, ..)| p.board && MARKER_RE.find_iter(&p.text).any(|m| ids_in(&GOALS, m.as_str()).contains(&id)));
    for (p, ..) in said.iter().filter(|(p, ..)| p.board) {
        theirs.extend(MARKER_RE.find_iter(&p.text).flat_map(|m| ids_in(&GOALS, m.as_str())));
    }
    // "It" with nothing else to mean: the conversation has this one goal of its own, or every goal it has
    // it made and this is the latest.
    let newest = made_goals.last() == Some(&id) && theirs.iter().all(|g| made_goals.contains(g));
    let only = theirs.len() == 1 && theirs.contains(&id);
    let prompts: Vec<GoalPrompt> = said
        .iter()
        .enumerate()
        .map(|(i, (p, made_here, before))| {
            let ours = ours || handed.is_some_and(|h| i < h);
            // What the owner's prompt before it asked the agent to make ("set up the emulator").
            let prompt = said.get(i + 1).filter(|(q, ..)| !q.boards()).and_then(|(q, ..)| prompt_topic(&owners_text(&q.text)));
            let own = owners_text(&p.text);
            let asks = |b: &str| asked_in(&GOALS, b, id, ours, only, prompt).contains(&id);
            // The question asks to run it; "now" picks the run now from a choice of when ("Should I run G2
            // now or after the release?"); the prompt quotes the question ("> Want me to run it?\nyes").
            let asked = before.as_deref().is_some_and(|b| {
                asks(b) || (NOW_ONLY_RE.is_match(own.trim()) && now_of_choice(b).is_some_and(|q| asks(&q)))
            }) || (i == 0 && quoted_ask(&p.text).is_some_and(|q| asks(&q)));
            let mut it = false;
            if i == 0 {
                // "It" is what was spoken of last: what the agent's message speaks of ("Wrote seed.sh.",
                // "The plan has a build step and a deploy step."), or, when it speaks of nothing ("Done."),
                // what the owner's prompt before it asked the agent to make. A message that ends on asking
                // about something else ("Ready to merge?") is what "ship it" answers.
                let spoke = before.as_deref().and_then(|b| antecedent(&CODE_RE.replace_all(b, "\n"), true));
                let reply_other = matches!(spoke, Some(Antecedent::Topic(Topic::Other))) || before.as_deref().is_some_and(asks_other);
                let only = (only || newest) && (spoke.is_some() || prompt != Some(Topic::Other));
                let asked_for = |p: &Prompt, made: &[i64]| !p.boards() && !p.clipped && made.last() == Some(&id) && MAKE_GOAL_RE.is_match(&owners_text(&p.text));
                // Only the word to go ("do it") answers whatever the agent just asked or offered: the run
                // only when it asked about that (`asked`), or asked nothing.
                let go = GOAL_GO_RE.is_match(own.trim()) && !before.as_deref().is_some_and(asks_any);
                let bare = GOAL_BARE_RE.is_match(own.trim()) || go;
                // "G2 looks good. run it": the goal the prompt names, and nothing else.
                let refs = refs_in(&own);
                let names = !refs.is_empty() && refs.iter().all(|r| *r == (true, id));
                let before = said.iter().skip(1).take(UNNAMED_REACH).position(|(_, m, _)| !m.is_empty()).map(|b| 1 + b);
                let quiet = |q: &Prompt| !q.boards() && !q.clipped && ACK_RE.is_match(owners_text(&q.text).trim());
                let earlier = before.is_some_and(|b| said[1..b].iter().all(|(q, ..)| quiet(q)) && asked_for(&said[b].0, &said[b].1));
                it = asked || names || asked_for(p, made_here) || (bare && !reply_other && (earlier || only));
            }
            GoalPrompt { prompt: p.clone(), ours, it, asked }
        })
        .collect();
    let fresh = &prompts[..prompts.len() - spent];
    Ok(goal_word_in(fresh, id).map(|i| fresh[i].prompt.text.clone()))
}

/// An agent's question about doing something else to a thing ("Ready to merge?", "Should I merge the PR?").
static OTHER_ASK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:merge|merging|merged|land|deploy|release|push|approve|close)\b").unwrap());

/// Whether the agent's message `reply` ends on a question about doing something else than a run ("The PR
/// for the footer is green. Ready to merge?").
fn asks_other(reply: &str) -> bool {
    let reply = CODE_RE.replace_all(reply, "\n");
    let last = SENTENCE_RE.captures_iter(&reply).filter(|c| c.name("s").is_some_and(|s| !s.as_str().trim().is_empty())).last();
    last.is_some_and(|c| {
        let q = &c["s"];
        c.name("end").is_some_and(|e| e.as_str().contains('?')) && OTHER_ASK_RE.is_match(q) && !GOALS.mention.is_match(q)
    })
}

/// Whether the agent's message `reply` asks the owner anything: a question, or an offer ("I can open a
/// PR if you want.").
fn asks_any(reply: &str) -> bool {
    let reply = CODE_RE.replace_all(reply, "\n");
    OFFER_RE.is_match(&reply) || SENTENCE_RE.captures_iter(&reply).any(|c| c.name("s").is_some_and(|s| !s.as_str().trim().is_empty()) && c.name("end").is_some_and(|e| e.as_str().contains('?')))
}

/// The end of the agent's message on a `reply` row, if the board has it: what the hook sent of its end
/// (`session_events.full`), or the message as it shows when it's shorter than the old hook's cut.
fn reply_end(row: &Row) -> Option<String> {
    if let Some(end) = row.s("full") {
        return Some(end.to_string());
    }
    let text = row.st("text");
    (text.chars().count() < OLD_HOOK_REPLY_KEEP && !text.ends_with('…')).then_some(text)
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
        ] {
            assert!(!says_start(no), "{no}");
            assert!(!read(no).held.is_empty(), "{no} holds the start");
        }
        // The agent is ready now: these put nothing off.
        for yes in ["start T8 when you're ready", "start T8 when you get a chance", "start T8 when you can"] {
            assert!(says_start(yes), "{yes}");
        }
    }

    #[test]
    fn a_question_is_no_ask() {
        for no in ["start T8?", "so, start T8?", "ok, queue it?", "start T8 now?", "start T8?!", "should we start T8 and T9?", "when will you start T8?", "can you start T8 tomorrow?"] {
            assert!(!says_start(no), "{no}");
            assert!(!read(no).held.is_empty(), "{no}");
        }
        // A polite ask in a question's shape asks.
        for yes in ["can you start T8?", "Could you please queue T8?", "would you kick off T8?", "hey, can you start T8?", "Will you start T8?"] {
            assert!(says_start(yes), "{yes}");
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
    fn the_agent_asking_to_start_a_task_is_read_from_its_last_question() {
        for yes in ["Should I start T8?", "Want me to kick off T8 now?", "T8 is ready. Want me to start it?", "Done.\n\nShall I queue T8?"] {
            assert!(asks_to_start(yes, 8), "{yes}");
        }
        for no in ["Should I start T9?", "Should I start T8 or T9?", "Want me to start T8 tomorrow?", "I made T8 and T9. Want me to start it?", "Should I start it?", "Should I not start T8?", "Start T8?\n\nDone."] {
            assert!(!asks_to_start(no, 8), "{no}");
        }
    }

    #[test]
    fn a_line_after_a_paste_takes_back_only_when_that_is_all_it_says() {
        for back in ["jk", "never mind", "nvm", "wait", "hold on", "actually no", "no.", "on second thought, don't"] {
            assert!(takes_back(back), "{back}");
        }
        for not in ["Wait timeout exceeded", "Hold on, retrying in 5s", "never mind the warnings above", "ok", "thanks"] {
            assert!(!takes_back(not), "{not}");
        }
        // #147: after filler, or going on with a no or a wait.
        for back in [
            "lol jk",
            "haha jk",
            "lol nvm",
            "hmm actually no",
            "never mind, don't start it",
            "wait, don't start it yet",
            "hold off on that",
            "scratch that lol",
            "ugh wait",
            "actually let's wait on that",
            "wait no, I need to check something first",
        ] {
            assert!(takes_back(back), "{back}");
        }
        for not in ["Wait for lock released", "Hold on: 3 retries left", "actually ran 41 jobs", "lol"] {
            assert!(!takes_back(not), "{not}");
        }
    }

    #[test]
    fn the_agent_asking_to_start_reads_lists_them_and_a_question_before_statements() {
        for yes in [
            "Should I start T8 and T9?",
            "Want me to start T8 and T9?",
            "I made T8 and T9. Want me to start them?",
            "Want me to start T8? It'll take a while.",
            "Should I start T8? Let me know.",
            "Should I start T8? (It only touches the footer.)",
            "Shall I start T8?\n\nIt only touches the footer.",
            "I can start T8 next. Want me to?",
        ] {
            assert!(asks_to_start(yes, 8), "{yes}");
        }
        assert!(asks_to_start("Should I start T8 and T9?", 9));
        for no in [
            "Want me to run T8's tests?",
            "Should I pick up T8's comments?",
            "Should I go for T8's approach?",
            "Should I start T8? Or should I wait?",
            "Should I start T8? I could also do it tomorrow.",
            "Should I start T8? Or T9.",
            "I made T8 and G2. Want me to start them?",
            "I can't start T8 yet. Want me to?",
        ] {
            assert!(!asks_to_start(no, 8), "{no}");
        }
    }

    #[test]
    fn a_yes_may_say_more_that_says_nothing_on_the_start() {
        for yes in ["y", "👍", "go", "please", "sure thing", "Yes. Also rename T9 to Footer.", "yes - and keep the PR small", "yep!"] {
            assert!(says_yes(&TASKS, yes, 8), "{yes}");
        }
        for no in ["thanks", "yes, but not now", "yes, tomorrow", "yes. Do it after the deploy.", "go fix the header first", "yes, but first fix the header", "ok, wait", "yes? why"] {
            assert!(!says_yes(&TASKS, no, 8), "{no}");
        }
    }

    #[test]
    fn asks_147_found_read_right() {
        for yes in ["start T8 when you're free", "would you mind starting T8?", "can u start T8", "CI's green - start T8", "start T8 when you can"] {
            assert_eq!(asks(yes), vec![Ask::Named(vec![8])], "{yes}");
        }
        for no in [
            "start T8 when you can confirm T9 passes",
            "start T8 when you can reproduce the bug",
            "start T8 when you get a chance to check T9",
            "start T8 when you're ready to merge T9",
            "would you start T8 or T9 first?",
            "copied from CI - start T8",
        ] {
            assert!(!says_start(no), "{no}");
        }
    }

    #[test]
    fn asks_151_found_read_right() {
        // A log line and a quote are no one's word; a take-back after a paste is the owner's.
        assert!(!takes_back("Wait - no response from the upstream host"));
        for back in ["oops, not that one", "lmao no", "wait - no, don't"] {
            assert!(takes_back(back), "{back}");
        }
        for no in ["the comment was - start T8", "the ticket's comment was - start T8"] {
            assert!(!says_start(no), "{no}");
        }
        for yes in ["the comment was fixed - start T8", "CI passed - start T8", "Will you start T8?"] {
            assert!(says_start(yes), "{yes}");
        }
        // A yes with an aside on something else, but not one on the start.
        for yes in ["k", "yes, no rush", "sure, no need to hurry", "yes. Don't touch the footer though.", "yes, and don't forget the migration", "yes, and ping me when the PR is up"] {
            assert!(says_yes(&TASKS, yes, 8), "{yes}");
        }
        for no in ["yes, but don't start yet", "yes, don't start it yet", "yes, not now", "yes, never mind"] {
            assert!(!says_yes(&TASKS, no, 8), "{no}");
        }
        assert!(names_only("just T8", 8) && !names_only("just T8", 9) && !names_only("just T8's tests", 8));
        // What the agent's question asks: a blocker after it, the nearest tasks, something of a task's.
        let asked = |reply: &str| asked_tasks(reply, 0);
        assert_eq!(asked("Should I start T8? T9 needs to merge first though."), Vec::<i64>::new());
        assert_eq!(asked("Want me to start T8? I'll open a draft PR when it's done."), vec![8]);
        assert_eq!(asked("Want me to start T8? I could also do it when T9 lands."), Vec::<i64>::new());
        assert_eq!(asked("T9 merged this morning. I made T10 and T11. Want me to start them?"), vec![10, 11]);
        assert_eq!(asked("I made T10 and T11 (T9 covers the rest). Want me to start them?"), vec![10, 11]);
        assert_eq!(asked("T8 is done. I made T10 for the follow-up. Want me to start it?"), vec![10]);
        assert_eq!(asked("I made two tasks:\n- T10 Colors\n- T11 Toggle\n\nWant me to start them?"), vec![10, 11]);
        for q in ["Should I start T8's review?", "Want me to start T8's preview env?", "Should I kick off T8's build?", "Should I queue T8's tests?"] {
            assert_eq!(asked(q), Vec::<i64>::new(), "{q}");
        }
    }

    #[test]
    fn what_was_spoken_of_last_is_what_it_means() {
        for goal in [
            "Done — two tasks, each with tests",
            "The goal has two tasks: add the migration and update the API",
            "Each task opens its own PR",
            "Two tasks: Colors (touches the theme files) and Toggle",
            "The plan has a build step and a deploy step",
            "Updated the plan",
        ] {
            assert_eq!(topic_of(goal), Some(Topic::Goal), "{goal}");
        }
        for other in [
            "I set up the dev database",
            "I wrote a rake task to backfill the colors",
            "The emulator is ready",
            "I built the app",
            "I wrote a small CLI to check the colors",
            "The codemod is ready",
            "I can run the linter on the plan files",
            "Wrote scripts/seed.sh",
        ] {
            assert_eq!(topic_of(other), Some(Topic::Other), "{other}");
        }
        for nothing in ["Done", "Done, it's ready", "Added", "Want me to "] {
            assert_eq!(topic_of(nothing), None, "{nothing}");
        }
        assert_eq!(prompt_topic("make a goal for adding tests to login"), Some(Topic::Goal));
        for other in ["set up the emulator", "write a backfill", "add a storybook story for the toggle", "write a script that seeds the db", "run the linter"] {
            assert_eq!(prompt_topic(other), Some(Topic::Other), "{other}");
        }
        for nothing in ["rename the second task", "also fix the header"] {
            assert_eq!(prompt_topic(nothing), None, "{nothing}");
        }
        // With nothing spoken of in the message, the owner's prompt decides.
        assert!(asked_in(&GOALS, "Done. Want me to run it?", 4, true, true, Some(Topic::Goal)).contains(&4));
        assert!(asked_in(&GOALS, "Done. Want me to run it?", 4, true, true, None).contains(&4));
        assert!(!asked_in(&GOALS, "Done. Want me to run it?", 4, true, true, Some(Topic::Other)).contains(&4));
        // The message speaks of the goal: the prompt doesn't veto it.
        assert!(asked_in(&GOALS, "The plan is ready. Want me to run it?", 4, true, true, Some(Topic::Other)).contains(&4));
    }

    #[test]
    fn it_in_the_agents_question_is_the_goal_only_with_no_nearer_noun() {
        for no in [
            "Wrote scripts/seed.sh. Want me to run it?",
            "I can run the linter on the plan files. Want me to run it?",
            "I wrote a quick benchmark. Should I run it?",
            "The docker container stopped. Should I start it?",
        ] {
            assert!(!asks_to_run(no, 4, true, true), "{no}");
        }
        for yes in ["Want me to run it? It has 2 tasks.", "Updated the plan. Want me to run it?"] {
            assert!(asks_to_run(yes, 4, true, true), "{yes}");
        }
        let goal = |t: &str| read_of(&GOALS, t).asks;
        assert_eq!(goal("ship it"), vec![Ask::It]);
        assert_eq!(goal("can you run it?"), vec![Ask::It]);
        assert_eq!(goal("start all of them"), vec![Ask::Unnamed]);
        for bare in ["let's run it", "can you run it?", "ship it"] {
            assert!(GOAL_BARE_RE.is_match(bare), "{bare}");
        }
    }

    #[test]
    fn the_agent_asking_to_run_the_goal_is_read_from_its_last_question() {
        for yes in ["Proposed two tasks for G4.\n\nShall I kick it off?", "Want me to run G4?", "Made G4. Want me to run it?", "Should I start the goal now?"] {
            assert!(asks_to_run(yes, 4, true, false), "{yes}");
        }
        for no in ["Want me to run G4, or tweak it?", "Want me to run G4 tomorrow?", "I added T1 to G4. Want me to start it?", "Want me to run G5?", "Should I open a PR?", "Run G4?\n\nDone."] {
            assert!(!asks_to_run(no, 4, true, false), "{no}");
        }
        // "It" with nothing named before it is the goal only when it's the conversation's one goal.
        for yes in ["The plan is ready. Want me to run it?", "Done, two tasks. Want me to run it?"] {
            assert!(asks_to_run(yes, 4, true, true), "{yes}");
            assert!(!asks_to_run(yes, 4, true, false), "{yes}");
        }
        assert!(!asks_to_run("I added T1. Want me to start it?", 4, true, true));
    }

    #[test]
    fn a_goal_run_is_asked_for_by_name_or_as_the_goal() {
        let goal = |t: &str| read_of(&GOALS, t);
        for yes in ["start G1", "run G1", "ok, run the goal", "start the goal", "kick off G1", "please queue goal G1", "run the goal G1 now", "You can start the goal"] {
            assert!(!goal(yes).asks.is_empty(), "{yes}");
        }
        for no in [
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
        // "It" is the goal only where the conversation says so (`GoalPrompt::it`).
        assert_eq!(goal("run it").asks, vec![Ask::It]);
        assert_eq!(goal("go ahead and run all the tasks").asks, vec![Ask::Unnamed]);
        // A goal ask is no word for a task, and the other way around.
        assert!(!says_start("start G1"));
        assert!(!says_start("start the goal"));
    }

    #[test]
    fn a_thing_is_its_names_last_word_and_a_describing_sentence_names_nothing_new() {
        // #155: whose, or a goal word before the name, only says which.
        for other in [
            "The plan's seed script is ready",
            "The goal's migration is written",
            "For the goal I wrote seed.sh",
            "Its first task needs a seed script",
            "In the goal there's a codemod",
            "For the plan I drafted a migration",
            "The emulator's ready",
        ] {
            assert_eq!(topic_of(other), Some(Topic::Other), "{other}");
        }
        for goal in ["I split the work into two tasks", "Here's a summary:", "Here's the plan:", "Updated the dark mode goal"] {
            assert_eq!(topic_of(goal), Some(Topic::Goal), "{goal}");
        }
        // #154: a sentence that only says more of what was named.
        for nothing in [
            "It only touches the header",
            "The fix is small",
            "Its PR will be small",
            "A quick look says it's one file",
            "The migration it needed merged",
            "The changes are small",
            "This touches the header and the footer",
            "My guess is a day of work",
            "I wrote it",
            "It has a Colors task and a Toggle task",
        ] {
            assert_eq!(topic_of(nothing), None, "{nothing}");
        }
        for (owners, want) in [
            ("make the plan's seed script", Topic::Other),
            ("add a goal-level smoke test", Topic::Other),
            ("write the goal's migration", Topic::Other),
            ("make a goal for the migration", Topic::Goal),
            ("plan the login tests as a goal", Topic::Goal),
            ("plan the dark mode goal", Topic::Goal),
        ] {
            assert_eq!(prompt_topic(owners), Some(want), "{owners}");
        }
        // "It" and "them": the tasks made, else the nearest named past what only describes them.
        let asked = |reply: &str| asked_tasks(reply, 0);
        assert_eq!(asked("I made T10. T8 is still in review. Want me to start it?"), vec![10]);
        assert_eq!(asked("I made T10 and T11. T8 is the old one. Want me to start them?"), vec![10, 11]);
        assert_eq!(asked("Made G4 with T10 and T11. Want me to start them?"), vec![10, 11]);
        assert_eq!(asked("T8 is ready. It only touches the header. Should I start it?"), vec![8]);
        assert_eq!(asked("T8 is ready. The dev server stopped. Should I start it?"), Vec::<i64>::new());
        assert!(asks_to_run("Made G4 with T10 and T11. Want me to run it?", 4, true, true));
        assert!(asks_to_run("Made G4:\n- T10 Colors\n- T11 Toggle\n\nWant me to run it?", 4, true, true));
        // Words after the question that set something before the start, and ones that don't.
        // #157: a hold word holds unless it's said not to or is about another subject.
        let t8 = [(false, 8)];
        for holds in [
            "T9 needs to merge first though",
            "It's blocked on T9",
            "I'd wait until T9 merges",
            "I need to check T9 first",
            "T9 goes first",
            "Let's do T9 first",
            "First, T9 needs to merge",
            "T9 blocks it",
            "It waits on T9",
        ] {
            assert!(holds_off(&TASKS, holds, &t8), "{holds}");
        }
        for not in [
            "It's the first of the two",
            "I'll write the tests first",
            "Nothing blocks it now",
            "T9 merged, so nothing is waiting on it",
            "I'll hold the PR as a draft until you look",
            "Colors goes first",
            "Nothing needs to happen first",
            "It should go first",
            "T9 is blocked on it",
            "T9 can wait",
            "No need to wait for T9 first",
            "The tests run before each commit",
            "It doesn't have to wait until T9 merges",
        ] {
            assert!(!holds_off(&TASKS, not, &t8), "{not}");
        }
        // A "don't start" that names nothing to start holds every start.
        assert!(read("start T8, don't start until T9 merges").asks.is_empty());
        assert!(!says_yes(&TASKS, "yes, don't start until T9 merges", 8));
        assert_eq!(read("start T8, don't start T9").asks, vec![Ask::Named(vec![8])]);
        // #157: a "don't start" on something else holds only that.
        for yes in ["yes, don't start T9", "yes, but don't start T9", "yes, don't start anything else", "yes, don't launch the emulator", "yes. Don't start T9 though."] {
            assert!(says_yes(&TASKS, yes, 8), "{yes}");
        }
        assert_eq!(read("start T8, don't start anything else").asks, vec![Ask::Named(vec![8])]);
        assert!(read("start T8, don't start anything").asks.is_empty());
    }

    #[test]
    fn the_latest_prompt_on_a_goal_run_decides() {
        let word = |ps: &[(&str, bool)], id| {
            goal_word_in(&ps.iter().map(|(t, o)| GoalPrompt { prompt: Prompt::new(t), ours: *o, ..GoalPrompt::default() }).collect::<Vec<_>>(), id)
        };
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
        assert_eq!(goal_word_in(&[GoalPrompt { prompt: clipped, ..GoalPrompt::default() }], 1), None, "its unread end may take it back");
        // "Run it" and a bare yes, only where the conversation says they mean the goal, and only latest.
        let g = |t: &str, it, asked| GoalPrompt { prompt: Prompt::new(t), it, asked, ..GoalPrompt::default() };
        assert_eq!(goal_word_in(&[g("run it", true, false)], 1), Some(0));
        assert_eq!(goal_word_in(&[g("run it", false, false)], 1), None);
        assert_eq!(goal_word_in(&[g("yes", false, true)], 1), Some(0));
        assert_eq!(goal_word_in(&[g("yes", false, false)], 1), None);
        // #157: an older yes stands, as an older ask does, but (#158) only with thanks after it, and not
        // further back than an unnamed ask reaches.
        assert_eq!(goal_word_in(&[g("thanks", false, false), g("yes", false, true)], 1), Some(1), "an older yes");
        assert_eq!(goal_word_in(&[g("thanks", false, true)], 1), None, "thanks is no yes");
        assert_eq!(goal_word_in(&[g("fix the header", false, false), g("yes", false, true)], 1), None, "a yes with more asked since");
        let thanks: Vec<GoalPrompt> = (0..=UNNAMED_REACH).map(|_| g("thanks", false, false)).chain([g("yes", false, true)]).collect();
        assert_eq!(goal_word_in(&thanks, 1), None, "a yes out of reach");
    }

    #[test]
    fn slips_and_dress_are_read_as_meant() {
        for (typo, word) in [("sart", "start"), ("strat", "start"), ("ahaed", "ahead"), ("yse", "yes"), ("yesss", "yes"), ("teh", "the"), ("pleae", "please")] {
            assert_eq!(fix_typo(typo), Some(word), "{typo}");
        }
        for real in ["star", "stars", "starts", "smart", "lunch", "head", "being", "doing", "then", "she", "run", "ok", "pleased"] {
            assert_eq!(fix_typo(real), None, "{real}");
        }
        assert_eq!(plain("**start T8**").trim(), "start T8");
        assert_eq!(plain("`T8` - start it"), "T8 - start it");
        assert_eq!(plain("👍👍"), "yes");
        assert_eq!(plain("🚀 run it"), "run it");
        assert_eq!(plain("can u start em w/ the fix plz"), "can you start them with the fix pls");
        assert_eq!(plain("## Plan\n- start T8\n- T9 can wait"), "\nstart T8\nT9 can wait");
    }
}
