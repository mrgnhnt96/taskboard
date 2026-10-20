//! The options of what a task waits on the owner for, kept in `tasks.ask` as JSON:
//! `{"picker": bool, "questions": [{"question", "header", "multi", "options": [{"label", "description"}]}]}`.
//!
//! Two places fill it: Claude's own question picker (`AskUserQuestion`, from the `PreToolUse` hook),
//! which the owner answers in the terminal or with the app's buttons (Midna presses them, see
//! [`picks`]), and `tb question --option`, whose options the app offers as one-click answers. `board::update_task` drops it with the question it belongs to.

use serde_json::{json, Value};

use crate::util::*;

const MAX_OPTIONS: usize = 8;
const MAX_QUESTIONS: usize = 4;

fn option(v: &Value) -> Option<Value> {
    let (label, description) = match v {
        // `tb question --option "Label: what it means"`.
        Value::String(s) => match s.split_once(": ") {
            Some((l, d)) => (l.to_string(), d.to_string()),
            None => (s.clone(), String::new()),
        },
        _ => (v["label"].as_str().unwrap_or("").to_string(), v["description"].as_str().unwrap_or("").to_string()),
    };
    let label = one_line(&label, 80);
    (!label.is_empty()).then(|| json!({"label": label, "description": one_line(&description, 300)}))
}

fn options(v: &Value) -> Vec<Value> {
    v.as_array().map(|a| a.iter().filter_map(option).take(MAX_OPTIONS).collect()).unwrap_or_default()
}

/// The ask for `tb question`'s options (strings "Label: what it means", or `{label, description}`),
/// or `None` without any.
pub fn from_options(question: &str, opts: Option<&Value>) -> Option<String> {
    let options = options(opts?);
    (!options.is_empty()).then(|| json!({"picker": false, "questions": [{"question": question, "options": options}]}).to_string())
}

/// Claude's picker (`AskUserQuestion`'s input): the line the board shows as the question, and the ask.
pub fn from_picker(input: &Value) -> Option<(String, String)> {
    let questions: Vec<Value> = input["questions"]
        .as_array()?
        .iter()
        .filter_map(|q| {
            let question = one_line(q["question"].as_str().unwrap_or(""), 500);
            (!question.is_empty()).then(|| {
                json!({"question": question, "header": one_line(q["header"].as_str().unwrap_or(""), 40),
                       "multi": q["multiSelect"] == true, "options": options(&q["options"])})
            })
        })
        .take(MAX_QUESTIONS)
        .collect();
    let first = questions.first()?["question"].as_str()?.to_string();
    let line = match questions.len() {
        1 => first,
        n => format!("{first} (1 of {n} questions)"),
    };
    Some((line, json!({"picker": true, "questions": questions}).to_string()))
}

/// A waiting task's picker, which Claude shows in its terminal.
pub fn is_picker(t: &Row) -> bool {
    t.s("status") == Some("needs") && parse(t)["picker"] == true
}

/// The owner's choices on a task's picker, one list of 0-based option indexes per question in its
/// order: Midna's `session.answer_question` answers (each with its question, which Midna checks the
/// picker shows) and the line that says them ("Apple; S, M"). A single-select question takes one.
pub fn picks(t: &Row, chosen: &Value) -> Result<(Value, String)> {
    let ask = parse(t);
    let questions = ask["questions"].as_array().cloned().unwrap_or_default();
    let chosen = chosen.as_array().filter(|c| c.len() == questions.len());
    let Some(chosen) = chosen else {
        return err(400, format!("Pick an answer for each of the picker's {} questions.", questions.len()));
    };
    let (mut answers, mut said) = (Vec::new(), Vec::new());
    for (q, c) in questions.iter().zip(chosen) {
        let opts = q["options"].as_array().cloned().unwrap_or_default();
        let idx: Option<Vec<usize>> = c.as_array().map(|c| c.iter().map(|i| i.as_u64().map(|i| i as usize).filter(|i| *i < opts.len())).collect()).unwrap_or(None);
        let Some(mut idx) = idx else {
            return err(400, format!("“{}” has no such option.", q["question"].as_str().unwrap_or("")));
        };
        idx.sort_unstable();
        idx.dedup();
        if idx.is_empty() || (q["multi"] != true && idx.len() != 1) {
            let n = if q["multi"] == true { "at least one option" } else { "one option" };
            return err(400, format!("Pick {n} for “{}”.", q["question"].as_str().unwrap_or("")));
        }
        said.push(idx.iter().map(|i| opts[*i]["label"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(", "));
        answers.push(json!({"options": idx, "question": q["question"]}));
    }
    Ok((json!(answers), said.join("; ")))
}

fn parse(t: &Row) -> Value {
    t.s("ask").and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null)
}

/// The ask on a task card: only while the task waits on the owner.
pub fn card(t: &Row) -> Value {
    if t.s("status") != Some("needs") {
        return Value::Null;
    }
    parse(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_from_strings_and_objects() {
        let a: Value = serde_json::from_str(&from_options("10 or 30?", Some(&json!(["10: The old limit", "30", "  "]))).unwrap()).unwrap();
        assert_eq!(a["picker"], false);
        assert_eq!(a["questions"][0]["question"], "10 or 30?");
        assert_eq!(a["questions"][0]["options"], json!([{"label": "10", "description": "The old limit"}, {"label": "30", "description": ""}]));
        assert!(from_options("q", Some(&json!([]))).is_none());
        assert!(from_options("q", None).is_none());
    }

    #[test]
    fn picker_questions() {
        let input = json!({"questions": [
            {"question": "Pick a fruit", "header": "Fruit", "multiSelect": false, "options": [{"label": "Apple", "description": "Red"}]},
            {"question": "Pick sizes", "header": "Size", "multiSelect": true, "options": [{"label": "S"}, {"label": "M"}]},
        ]});
        let (line, ask) = from_picker(&input).unwrap();
        assert_eq!(line, "Pick a fruit (1 of 2 questions)");
        let a: Value = serde_json::from_str(&ask).unwrap();
        assert_eq!(a["picker"], true);
        assert_eq!(a["questions"][1]["multi"], true);
        assert_eq!(a["questions"][0]["options"][0], json!({"label": "Apple", "description": "Red"}));
        assert!(from_picker(&json!({"questions": []})).is_none());
    }
}
