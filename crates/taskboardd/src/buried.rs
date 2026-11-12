//! Questions an agent buries mid-reply. The board reads a turn's question to the owner only from its
//! reply's last paragraph (`reports::ends_on_question`), so one asked above it never becomes a
//! Question card. When a task's turn ends that way, the Stop hook sends it back once to ask it with
//! `tb question` or in a final Questions block. A board setting (on unless the owner switches it off)
//! turns the check off.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;

const OFF_KEY: &str = "buried_questions_off";

/// The check is on: the owner hasn't switched it off.
pub fn on(app: &App) -> bool {
    app.db.get_setting(OFF_KEY).ok().flatten().as_deref() != Some("1")
}

/// `GET /buried-questions`.
pub fn settings(app: &App) -> Result<Value> {
    Ok(json!({"on": on(app)}))
}

/// `POST /buried-questions {on}`: `on` is true/false or "on"/"off".
pub fn set_settings(app: &App, body: &Value) -> Result<Value> {
    let want = match body.get("on") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) if s == "on" => true,
        Some(Value::String(s)) if s == "off" => false,
        _ => return err(400, "Say on: true or false."),
    };
    app.db.set_setting(OFF_KEY, if want { None } else { Some("1") })?;
    settings(app)
}

/// The reply with what isn't the agent asking left out: fenced code, inline code and quoted lines.
fn own_words(reply: &str) -> String {
    let mut fence: Option<&str> = None;
    let mut kept = vec![];
    for line in reply.lines() {
        let t = line.trim_start();
        let mark = ["```", "~~~"].into_iter().find(|m| t.starts_with(m));
        match (fence, mark) {
            (None, Some(m)) => {
                fence = Some(m);
                kept.push(String::new());
            }
            (Some(f), Some(m)) if f == m && t.trim_start_matches(&m[..1]).trim().is_empty() => {
                fence = None;
                kept.push(String::new());
            }
            (Some(_), _) => kept.push(String::new()),
            (None, None) if t.starts_with('>') => kept.push(String::new()),
            (None, None) => kept.push(INLINE_CODE.replace_all(line, "").into_owned()),
        }
    }
    kept.join("\n")
}

static INLINE_CODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`+[^`\n]*`+").unwrap());

/// The question a reply asks the owner above its last paragraph, when its last paragraph asks
/// nothing: `reply` is the whole reply, `end` its end (what `ends_on_question` reads).
pub fn buried_question(reply: &str, end: &str) -> Option<String> {
    if crate::reports::ends_on_question(end).is_some() {
        return None;
    }
    own_words(reply).split("\n\n").map(str::trim).find(|p| !p.is_empty() && crate::reports::asks_owner(p)).map(|p| one_line(p, 300))
}

/// What the Stop hook tells an agent it sends back.
pub fn reason(app: &App, asked: &str, tb: &str) -> String {
    let owner = &app.cfg.owner;
    format!(
        "Task board: your reply asks {owner} something above its last paragraph (“{asked}”), where the board doesn't see it, so it never reaches {owner}. \
         Ask it with {tb} question \"<the question>\" --option \"<choice>: <what it means>\" (one --option per choice) and stop, \
         or move it into a final Questions block at the end of your reply. If it wasn't a question for {owner}, end your turn again as it is."
    )
}

#[cfg(test)]
mod tests {
    use super::buried_question;

    fn buried(reply: &str) -> Option<String> {
        buried_question(reply, reply)
    }

    #[test]
    fn a_question_above_the_last_paragraph_is_buried() {
        assert_eq!(buried("Should I keep the old endpoint?\n\nI capped the retries at 3."), Some("Should I keep the old endpoint?".into()));
        assert!(buried("I read the loop.\n\nWant me to split the parser too\n\nNext I add the tests.").is_some());
        assert!(buried("Here it is.\n\nLet me know which name you like.\n\nDone for now.").is_some());
    }

    #[test]
    fn a_question_in_the_last_paragraph_is_not_buried() {
        assert_eq!(buried("I capped the retries.\n\nShould the cap be 3 or 5?"), None);
        assert_eq!(buried("Is 3 right?\n\n## Questions\n\n1. Should the cap be 3 or 5?"), None);
    }

    #[test]
    fn code_and_quotes_ask_nothing() {
        assert_eq!(buried("It reads:\n\n```rust\nlet x = a?;\nif ok? { }\n```\n\nDone."), None);
        assert_eq!(buried("The flag `--retry?` stays.\n\nDone."), None);
        assert_eq!(buried("You asked:\n> Should I keep it?\n\nI kept it."), None);
        assert_eq!(buried("~~~\nwhy?\n~~~\n\nDone."), None);
        assert!(buried("```\ncode\n```\nShould I ship it?\n\nDone.").is_some(), "the question after the fence counts");
    }

    #[test]
    fn a_reply_that_asks_nothing_is_not_buried() {
        assert_eq!(buried("I capped the retries at 3.\n\nNext I test it."), None);
        assert_eq!(buried("See https://example.com/a?b=1 for the docs.\n\nDone."), None);
    }
}
