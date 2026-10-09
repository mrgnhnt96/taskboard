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
    let checks = if skipped.is_some() {
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
    let approvals = rec["approvals"].as_i64().unwrap_or(0);
    let reviewers = approvals + rec["requested"].as_i64().unwrap_or(0);
    let new_comments = (rec["comments"].as_i64().unwrap_or(0) - f.i0("comments_seen")).max(0);
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
        "reviewer_rows": [],
    }))
}
