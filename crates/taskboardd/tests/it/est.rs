//! Estimates for tests that don't care about them: `tb task new`, `tb goal new` and `tb propose`
//! refuse agent work without one, so helpers that build those bodies fill in an hour.

use serde_json::{json, Value};

fn item(v: &Value) -> Value {
    match v {
        Value::String(s) if !s.trim().is_empty() && !s.contains("::est=") => json!(format!("{s}::est=1h")),
        Value::Object(o) if o.get("est").is_none() && o.get("kind").and_then(Value::as_str) != Some("human") => {
            let mut o = o.clone();
            o.insert("est".into(), json!("1h"));
            Value::Object(o)
        }
        _ => v.clone(),
    }
}

/// The report `body` with an estimate on each piece of agent work that lacks one.
pub fn fill(mut body: Value) -> Value {
    let event = body["event"].as_str().unwrap_or("").to_string();
    if matches!(event.as_str(), "tb.goal" | "tb.propose") {
        if let Some(ts) = body.get_mut("tasks").and_then(Value::as_array_mut) {
            for t in ts.iter_mut() {
                *t = item(t);
            }
        }
    }
    let agent = body["kind"].as_str() != Some("human");
    if (event == "tb.goal" || (event == "tb.new_task" && agent)) && body.get("est").is_none() {
        body["est"] = json!("1h");
    }
    body
}
