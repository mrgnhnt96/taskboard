//! The owner's word that human tasks are done (`tb needs done H3 H4`). Only the owner says a human task
//! is done: by pressing Done in the app, or by telling an agent so in its terminal. The board doesn't take
//! the agent's say-so: it reads the prompts the owner typed there, as it reads a start word
//! ([`crate::startword`]).
//!
//! A done word asks to mark one done ("mark H4 done", "mark them as done") or says it is ("H3 and H4 are
//! done", "those are done", "I did H4", "H4 done", or only "done"). Each is read as the ask "mark … done",
//! so a question, a no, a time or a condition holds it back as it holds a start ("is H4 done?", "H4 isn't
//! done", "mark H4 done tomorrow", "once the email comes, mark it done"), and so does a hedge ("I think
//! H4 is done"). A done word that names no task ("them", "both", "it") is for the human tasks the
//! conversation talked over up to that prompt: "it" only when there's one, or when the agent's message
//! just before named one.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::app::App;
use crate::p;
use crate::startword::{owners_text, read_of, typed, Ask, Kind, Prompt, BOARD_PROMPT, LISTED, MADE, NAMED};
use crate::util::*;

/// A human task's ref ("H4"), which reads as its T ref: the same task.
static H_REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[Hh](\d+)\b").unwrap());
/// "To-do", the owner's word for a human task.
static TODO_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bto-?do(s?)\b").unwrap());

/// What a done word may say of tasks it doesn't name: "them", "both of them", "all the tasks", "it".
const UNNAMED: &str = concat!(
    r"(?:(?:both|all|each)\s+of\s+(?:them|these|those|the\s+(?:human\s+)?(?:tasks|ones))|them\s+(?:all|both)|",
    r"all\s+(?:of\s+)?(?:the\s+|these\s+|those\s+|my\s+|your\s+)?(?:human\s+)?(?:tasks|ones)|",
    r"(?:the|this|that|these|those|my|your|our)\s+(?:human\s+)?(?:one|ones|task|tasks)|it|this|that|them|these|those|both)"
);
/// Who a statement says is done: the tasks it names, or "they", "both", "it", "those tasks".
static WHO: Lazy<String> = Lazy::new(|| format!(r"(?P<who>{NAMED}|they|{UNNAMED})"));
/// The words for done in an ask ("mark it done").
const DONE: &str = r"(?:done|complete|completed|finished)";
/// The words for done in a statement ("it's done", "both are sorted").
const DONE_SAID: &str = r"(?:done|complete|completed|finished|handled|sorted(?:\s+out)?|taken\s+care\s+of)";
/// Words between "is" and done that add nothing ("they're all done", "H4 is now done").
const FILL: &str = r"(?:\s+(?:all|both|each|now|already|finally|actually|also|just|really|totally|fully))*";

/// Marks of human tasks as done (`tb needs done`), read with H refs as T refs: "mark T4 done", "mark
/// them as done", "mark both complete", "mark as done T3 and T4".
pub static DONES: Lazy<Kind> = Lazy::new(|| {
    let on = format!(
        r"(?:\bmark\s+(?:{LISTED}(?P<named>{NAMED})|(?P<unnamed>{UNNAMED}))\s+(?:as\s+)?{DONE}|\bmark\s+(?:as\s+)?{DONE}\s+(?:{LISTED}(?P<named2>{NAMED})|(?P<unnamed2>{UNNAMED})))"
    );
    Kind { mention: Regex::new(&format!(r"(?i){on}\b")).unwrap(), ids: Regex::new(r"\b[Tt](\d+)\b").unwrap(), goal: false }
});

/// "I haven't done T4", "we didn't finish them".
static NOT_DID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?:i|we)\s+(?:haven[’']?t|have\s+not|didn[’']?t|did\s+not|never)(?:\s+(?:yet|actually|really|quite))*\s+(?:done|did|do|finished|finish|completed|complete|handled|handle)\s+{}\b",
        *WHO
    ))
    .unwrap()
});
/// "I did T4", "I've finished both", "we took care of them".
static DID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?:i|we)(?:[’']ve|\s+have)?(?:\s+(?:just|already|now|finally|also|actually))*\s+(?:did|done|finished|completed|handled|sorted(?:\s+out)?|took\s+care\s+of|taken\s+care\s+of)\s+{}\b",
        *WHO
    ))
    .unwrap()
});
/// "T4 isn't done", "they're not finished yet".
static NOT_SAID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        r"(?i){}\s*(?:(?:is|are|was|were)(?:n[’']?t|\s+not)|[’'](?:s|re)\s+not|(?:has|have)(?:n[’']?t|\s+not)\s+been){FILL}\s+(?:yet\s+)?{DONE_SAID}\b",
        *WHO
    ))
    .unwrap()
});
/// "Is T4 done?", "are they finished".
static ASKED_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(r"(?i)\b(?:is|are|was|were|has|have)\s+{}(?:\s+been)?{FILL}\s+{DONE_SAID}\b", *WHO)).unwrap()
});
/// "T4 is done", "they're both finished", "those are handled".
static SAID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(r"(?i){}\s*(?:(?:is|are|was|were|has\s+been|have\s+been)\b|[’'](?:s|re)){FILL}\s+{DONE_SAID}\b", *WHO)).unwrap()
});
/// "Done with T4".
static DONE_WITH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(&format!(r"(?i)\b(?:all\s+)?(?:done|finished)\s+with\s+{}\b", *WHO)).unwrap());
/// "T4 done", "T3, T4: done", "T4 - done".
static TERSE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(&format!(r"(?i)(?P<who>\b{NAMED})\s*(?:[:—–]|\s-+)?\s*(?:all\s+|both\s+)?{DONE}\b")).unwrap());
/// A mark just before the tasks: what [`TERSE_RE`] found is an ask already ("mark T4 done").
static MARK_BEFORE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(&format!(r"(?i)\bmark\s+(?:as\s+)?{LISTED}$")).unwrap());
/// A prompt that's only a done ("done", "all done", "both finished", "did it", "yes, done").
static BARE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(?:(?:ok(?:ay)?|yes|yep|yeah|yup|alright|great|and)[\s,.!]*)*(?:(?P<all>all|both)\s+)?(?:done|finished|completed|did\s+(?:it|that|(?P<them>them|both|those)))(?:\s+(?:now|thanks|thank\s+you))?[\s.!]*$").unwrap()
});
/// What before a statement, in its sentence, makes it a guess ("I think T4 is done", "maybe they're
/// finished").
static HEDGE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:think|thought|guess|maybe|probably|might|hope|hopefully|believe|perhaps|possibly|pretty\s+sure|should|assume|suppose)\b").unwrap()
});
/// Where the sentence a statement is in begins.
static SENTENCE_START_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[.!?;\n…]").unwrap());

/// "They" as an ask says it: "them".
fn as_ask(who: &str) -> &str {
    if who.eq_ignore_ascii_case("they") {
        "them"
    } else {
        who
    }
}

/// `text` with every statement of done said as its ask: "T4 is done" as "mark T4 done", "T4 isn't done"
/// as "don't mark T4 done", "is T4 done?" and "I think T4 is done" as asks about it. H refs read as T refs.
pub fn as_asks(text: &str) -> String {
    let text = H_REF_RE.replace_all(text, "T$1");
    let text = TODO_RE.replace_all(&text, "task$1");
    let said = |re: &Regex, text: &str, ask: &str| -> String {
        re.replace_all(text, |c: &regex::Captures| {
            let at = c.get(0).map_or(0, |m| m.start());
            let lead = &text[SENTENCE_START_RE.find_iter(&text[..at]).last().map_or(0, |m| m.end())..at];
            let ask = if ask == "mark" && HEDGE_RE.is_match(lead) { "should I mark" } else { ask };
            format!("{ask} {} done", as_ask(&c["who"]))
        })
        .into_owned()
    };
    let text = said(&NOT_DID_RE, &text, "don't mark");
    let text = said(&DID_RE, &text, "mark");
    let text = said(&NOT_SAID_RE, &text, "don't mark");
    let text = said(&ASKED_RE, &text, "did you mark");
    let text = said(&SAID_RE, &text, "mark");
    let text = said(&DONE_WITH_RE, &text, "mark");
    let text = TERSE_RE
        .replace_all(&text, |c: &regex::Captures| {
            let at = c.get(0).map_or(0, |m| m.start());
            if MARK_BEFORE_RE.is_match(&text[..at]) {
                c[0].to_string()
            } else {
                format!("mark {} done", &c["who"])
            }
        })
        .into_owned();
    bare(&text).map_or(text, |a| a.into())
}

/// A prompt that's only a done ("done", "all done", "did it") as its ask: "mark it done" or "mark them done".
fn bare(text: &str) -> Option<&'static str> {
    BARE_RE.captures(owners_text(text).trim()).map(|c| if c.name("all").is_some() || c.name("them").is_some() { "mark them done" } else { "mark it done" })
}

/// One prompt of the conversation, with the human tasks it may mean without naming them.
#[derive(Debug, Clone, Default)]
pub struct Said {
    pub prompt: Prompt,
    /// The human tasks the conversation talked over up to and in this prompt: named (as H refs) in a
    /// prompt or the agent's messages, or made by it.
    pub talked: Vec<i64>,
    /// The human tasks the agent's message just before this prompt names.
    pub named: Vec<i64>,
}

/// Which prompt in `said` (newest first) is the owner's word that human task `id` is done. The latest
/// prompt that speaks of it decides: a no, a question, a time or a condition there is no word. A done
/// word that names no task means the tasks talked over up to it ([`Said::talked`]): "them" every one,
/// "it" the one the agent's message just before named, or the only one talked over. A prompt that's only a
/// done ("done", "all done") means only the tasks the agent's message just before it named.
pub fn word_in(said: &[Said], id: i64) -> Option<usize> {
    for (i, s) in said.iter().enumerate() {
        let p = &s.prompt;
        if p.boards() {
            continue;
        }
        // A bare "done" answers the agent's message before it: it means only what that message named.
        let talked = if bare(&p.text).is_some() { &s.named } else { &s.talked };
        let unnamed = |a: &Ask| match a {
            Ask::Them | Ask::First => talked.contains(&id),
            Ask::Unnamed | Ask::It if s.named.is_empty() => talked == &[id],
            Ask::Unnamed | Ask::It => s.named == [id],
            Ask::Named(_) => false,
        };
        match read_of(&DONES, &as_asks(&p.text)).word_for(id, unnamed) {
            Some(true) if p.clipped => {}
            Some(true) => return Some(i),
            Some(false) => return None,
            None => {}
        }
    }
    None
}

/// The human tasks `text` names by their H ref.
fn h_refs(text: &str) -> Vec<i64> {
    H_REF_RE.captures_iter(text).filter_map(|c| c[1].parse().ok()).collect()
}

fn add(to: &mut Vec<i64>, ids: impl IntoIterator<Item = i64>) {
    for id in ids {
        if !to.contains(&id) {
            to.push(id);
        }
    }
}

/// Whether `id` is a human task on the board.
fn human(app: &App, id: i64) -> Result<bool> {
    Ok(crate::board::find_task(app, Some(id))?.is_some_and(|t| crate::human::is(&t)))
}

/// The prompts of terminal `sid` since its Claude conversation began, newest first, each with the human
/// tasks talked over up to it and those the agent's message before it named.
fn said_in(app: &App, sid: &str) -> Result<Vec<Said>> {
    let rows = app.db.q(
        "SELECT id, at, kind, text, full, data FROM session_events WHERE session_id = ? AND kind IN ('prompt', 'reply', ?)
           AND id > COALESCE((SELECT MAX(id) FROM session_events WHERE session_id = ? AND kind = 'start'), 0)
         ORDER BY id",
        p![sid, MADE, sid],
    )?;
    let (mut said, mut talked, mut named): (Vec<Said>, Vec<i64>, Vec<i64>) = (vec![], vec![], vec![]);
    for r in &rows {
        match r.st("kind").as_str() {
            "reply" => {
                named = h_refs(&format!("{}\n{}", r.st("text"), r.st("full")));
                add(&mut talked, named.iter().copied());
            }
            k if k == MADE => {
                if let Some(id) = r.st("data").parse().ok().filter(|&id| human(app, id).unwrap_or(false)) {
                    add(&mut talked, [id]);
                }
            }
            _ => {
                let (text, clipped) = typed(r);
                add(&mut talked, h_refs(&text));
                let prompt = Prompt { text, clipped, board: r.s("data") == Some(BOARD_PROMPT), at: r.st("at"), ..Prompt::default() };
                said.push(Said { prompt, talked: talked.clone(), named: std::mem::take(&mut named) });
            }
        }
    }
    said.reverse();
    Ok(said)
}

/// The owner's word in terminal `sid` that human task `id` is done ([`word_in`]): the prompt that says so.
pub fn owners_word(app: &App, sid: &str, id: i64) -> Result<Option<String>> {
    if sid.is_empty() {
        return Ok(None);
    }
    let said = said_in(app, sid)?;
    Ok(word_in(&said, id).map(|i| said[i].prompt.text.clone()))
}

/// What to tell the agent about the latest prompt in terminal `sid` (`owners`: the owner typed it): the
/// open human tasks it's the word that are done, or, for one that holds a done back, that it's no word.
pub fn note_prompt(app: &App, sid: &str, owners: bool) -> Result<Option<String>> {
    if !owners {
        return Ok(None);
    }
    let said = said_in(app, sid)?;
    let Some(latest) = said.first() else { return Ok(None) };
    let text = as_asks(&latest.prompt.text);
    if !DONES.mention.is_match(&owners_text(&text)) {
        return Ok(None);
    }
    let r = read_of(&DONES, &text);
    let mut maybe = latest.talked.clone();
    for a in &r.asks {
        if let Ask::Named(ids) = a {
            add(&mut maybe, ids.iter().copied());
        }
    }
    let mut covered = vec![];
    for id in maybe {
        let open = crate::board::find_task(app, Some(id))?.is_some_and(|t| crate::human::is(&t) && t.s("status") != Some("done"));
        if open && word_in(&said, id) == Some(0) {
            covered.push(format!("H{id}"));
        }
    }
    if !covered.is_empty() {
        let list = covered.join(" ");
        return Ok(Some(format!(
            "Task board: this prompt is the owner's word that a human task is done, for {list}: run tb needs done with the ones they mean (tb needs done {list})."
        )));
    }
    Ok(r.asks.is_empty().then(|| {
        "Task board: this prompt is not the owner's word that a human task is done: it asks, says no, guesses, or sets a time or condition, so don't run tb needs done on it.".to_string()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_statement_of_done_reads_as_its_ask() {
        for (said, ask) in [
            ("H4 is done", "mark T4 done"),
            ("H3 and H4 are done.", "mark T3 and T4 done."),
            ("they're all done", "mark them done"),
            ("those are done", "mark those done"),
            ("I did H4", "mark T4 done"),
            ("I've finished both", "mark both done"),
            ("H4 isn't done yet", "don't mark T4 done yet"),
            ("I haven't done H4", "don't mark T4 done"),
            ("is H4 done?", "did you mark T4 done?"),
            ("I think H4 is done", "I think should I mark T4 done"),
            ("H4 done", "mark T4 done"),
            ("mark H4 done", "mark T4 done"),
            ("mark the to-dos as done", "mark the tasks as done"),
            ("done", "mark it done"),
            ("all done!", "mark them done"),
            ("did it", "mark it done"),
        ] {
            assert_eq!(as_asks(said), ask, "{said}");
        }
    }
}
