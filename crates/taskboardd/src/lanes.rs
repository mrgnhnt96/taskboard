//! The Tasks page (`GET /lanes`): every task outside a goal (or every task, with `goals=1`) that is
//! queued, running, needs you or done today, one lane each, plus the projects they're in.
//!
//! Queued runs in the runner's order (high priority first, then oldest), with the ones already
//! starting first. Done is what finished today plus anything done whose PR is still open. Planned
//! tasks never show: nothing runs them until the owner starts them.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, p};

const LANES: [&str; 4] = ["queued", "running", "needs", "done"];

fn finished_today(t: &Row) -> bool {
    let at = t.s("finished_at").or(t.s("updated_at")).and_then(parse_iso);
    at.is_some_and(|ts| local_dt(ts).date_naive() == local_now().date_naive())
}

fn lane_of(t: &Row) -> Option<&'static str> {
    match t.s("status") {
        Some("queued") => Some("queued"),
        Some("working") => Some("running"),
        Some("needs") => Some("needs"),
        Some("done") if finished_today(t) || board::pr_still_open(t) => Some("done"),
        _ => None,
    }
}

/// The runner's order (`runner::start_queued`), the ones already starting first.
fn queued_key(t: &Row) -> (bool, bool, String, i64) {
    (t.i("start_job").is_none(), t.s("priority") != Some("high"), t.st("created_at"), t.id())
}

/// Newest first, by when it last moved (`started_at` while running, `finished_at` once done).
fn recent_key(t: &Row) -> String {
    t.s("finished_at").or(t.s("started_at")).or(t.s("updated_at")).unwrap_or("").to_string()
}

pub fn page(app: &App, with_goals: bool) -> Result<Value> {
    let sql = if with_goals { "SELECT * FROM tasks ORDER BY id" } else { "SELECT * FROM tasks WHERE goal_id IS NULL ORDER BY id" };
    let rows = app.db.q(sql, p![])?;
    let mut lanes: Vec<(&str, Vec<&Row>)> = LANES.iter().map(|l| (*l, vec![])).collect();
    for t in &rows {
        if let Some(l) = lane_of(t) {
            if let Some((_, v)) = lanes.iter_mut().find(|(k, _)| *k == l) {
                v.push(t);
            }
        }
    }
    let mut projects: Vec<(String, i64)> = vec![];
    let mut out = serde_json::Map::new();
    for (k, mut v) in lanes {
        if k == "queued" {
            v.sort_by_key(|t| queued_key(t));
        } else {
            v.sort_by_key(|t| std::cmp::Reverse(recent_key(t)));
        }
        let mut cards = vec![];
        for t in &v {
            let name = t.st("project");
            match projects.iter_mut().find(|(p, _)| *p == name) {
                Some((_, n)) => *n += 1,
                None => projects.push((name, 1)),
            }
            cards.push(board::task_card(app, t)?);
        }
        out.insert(k.to_string(), Value::Array(cards));
    }
    projects.sort();
    out.insert("projects".into(), json!(projects.iter().map(|(name, count)| json!({"name": name, "count": count})).collect::<Vec<_>>()));
    Ok(Value::Object(out))
}
