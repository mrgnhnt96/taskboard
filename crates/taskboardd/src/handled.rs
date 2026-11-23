//! What the board took off the owner's Needs you list by itself, for the app's Handled for you list:
//! each is a task event of kind [`KIND`], logged by the board, whose data says what did it (`by`),
//! what the item was (`item`), the verdict and what Pull back needs to put it back.

use serde_json::{json, Value};

use crate::app::App;
use crate::board;
use crate::util::*;

pub const KIND: &str = "handled";

/// Records that the board took task `task_id`'s item off Needs you. `by` names the sweep that did it
/// (`recheck`, `retry`, `misses`); `data` holds the rest, such as `item` and `verdict`.
pub fn record(app: &App, task_id: i64, by: &str, text: &str, data: Value) -> Result<i64> {
    let mut d = json!({"by": by});
    if let Value::Object(more) = data {
        for (k, v) in more {
            d[k] = v;
        }
    }
    board::log_event_full(app, task_id, board::BOARD, KIND, text, Some(d), None)
}
