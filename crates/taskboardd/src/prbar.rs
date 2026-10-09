//! What the app's PR bar shows for a task's PR beyond its stage: the build's link, whether checks are
//! needed or this PR's, the owner's own look, the review count, new comments, and the PR it stacks on.
//! The author-side review step (`bar` in `[[steps]]`) and reviewer rows go in `wd` and `reviewers`.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{prflow, stack};

/// The link of the check to look at: the first failed one, else the first with a link.
pub fn build_url(rec: &Value) -> Value {
    let checks = rec["checks"].as_array().cloned().unwrap_or_default();
    let link = |c: &Value| c["url"].as_str().filter(|u| u.starts_with("http")).map(|u| u.to_string());
    checks
        .iter()
        .filter(|c| c["state"] == "failed")
        .find_map(link)
        .or_else(|| checks.iter().find_map(link))
        .map(Value::String)
        .unwrap_or(Value::Null)
}

/// `pr.bar` on a task card with a PR.
pub fn bar(app: &App, t: &Row) -> Result<Value> {
    let f = jloads_obj(t.s("pr_flow"));
    let rec = f.get("rec").cloned().unwrap_or(json!({}));
    let head = rec["head"].as_str().unwrap_or("");
    let skipped = prflow::checks_skipped(&f, &rec);
    let cleared = !prflow::not_ours(&f, &rec).is_null() && prflow::failing(&f, &rec).is_empty() && rec["failed"].as_array().is_some_and(|a| !a.is_empty());
    let checks = if skipped.is_some() || cleared {
        "not_ours"
    } else if rec["checks"].as_array().map(|a| a.is_empty()).unwrap_or(false) && rec.get("state").is_some() {
        "not_needed"
    } else {
        ""
    };
    let review_skipped = f.get("skip_review").and_then(|v| v.as_object()).map(|m| m.contains_key(head) || m.contains_key("*")).unwrap_or(false);
    let you = if f.contains_key("reviewed") {
        "reviewed"
    } else if review_skipped {
        "skipped"
    } else if t.s("status") == Some("done") && prflow::awaiting_owner(t) {
        "waiting"
    } else {
        ""
    };
    let review = prflow::review_of(&f, &rec);
    let approvals = review.approvals;
    let rows = reviewer_rows(&f, &rec);
    // "x of N": the host's reviewer list when it has one, else approvals plus the reviewers still asked.
    let reviewers = if rec["reviewers"].is_array() { rows.len() as i64 } else { approvals + rec["requested"].as_i64().unwrap_or(0) };
    let new_comments = if rec["threads"].is_array() {
        prflow::open_threads(&f, &rec).len() as i64
    } else {
        (rec["comments"].as_i64().unwrap_or(0) - f.i0("comments_seen")).max(0)
    };
    Ok(json!({
        "build_url": build_url(&rec),
        "checks": if checks.is_empty() { Value::Null } else { json!(checks) },
        "checks_why": skipped,
        "you": if you.is_empty() { Value::Null } else { json!(you) },
        "approvals": approvals, "reviewers": reviewers,
        "new_comments": new_comments,
        "waits_on_base": t.s("pr_phase") == Some("waits"),
        "stacks_on": stack::card(app, t)?,
        "retargeted": f.get("retargeted").cloned().unwrap_or(Value::Null),
        "wd": Value::Null,
        "reviewer_rows": rows,
    }))
}

/// One pill per reviewer still on the PR (swapped-off ones aside), from the host's reviewer states:
/// approved, changes (asked for changes), rereview (asked again after `tb pr addressed`), waiting
/// (asked, hasn't reviewed) or commented. `swaps` counts reviewer swaps (0 until swapping lands).
pub fn reviewer_rows(f: &Row, rec: &Value) -> Vec<Value> {
    let off = prflow::swapped_off(f);
    let asked_again = str_list(&f.get("addressed").map(|a| a["asked_ids"].clone()).unwrap_or(Value::Null));
    rec["reviewers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|r| !off.iter().any(|o| r["user"].as_str() == Some(o.as_str())))
        .map(|r| {
            let user = r["user"].as_str().unwrap_or("");
            let again = r["requested"] == true && asked_again.iter().any(|u| u == user);
            let state = match r["state"].as_str().unwrap_or("") {
                "approved" => "approved",
                "changes" if again => "rereview",
                "changes" => "changes",
                "commented" => "commented",
                _ if again => "rereview",
                _ => "waiting",
            };
            let name = r["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(user);
            json!({"name": name, "user": user, "state": state, "swaps": 0})
        })
        .collect()
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default()
}
