//! What the app's PR bar shows for a task's PR beyond its stage: the build's link, whether checks are
//! needed or this PR's, the owner's own look, the review count, new comments, and the PR it stacks on.
//! The author-side review step (`bar` in `[[steps]]`) and reviewer rows go in `wd` and `reviewers`.

use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{prflow, stack};

/// Where the Checks step links: the newest build by its `at` (a failed one, else a running one, else
/// any), else the PR's own list of checks (GitHub's Checks tab; elsewhere the PR). Checks with no
/// time count as older than ones with; among equal times, the last the host lists is newest.
pub fn build_url(rec: &Value, pr_url: Option<&str>) -> Value {
    let checks = rec["checks"].as_array().cloned().unwrap_or_default();
    let link = |c: &Value| c["url"].as_str().filter(|u| u.starts_with("http")).map(|u| u.to_string());
    let newest = |state: Option<&str>| {
        checks
            .iter()
            .enumerate()
            .filter(|(_, c)| state.is_none_or(|s| c["state"] == s))
            .filter_map(|(i, c)| Some((c["at"].as_str().and_then(parse_iso).unwrap_or(f64::NEG_INFINITY), i, link(c)?)))
            .max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, _, u)| u)
    };
    let failed = newest(Some("failed")).or_else(|| newest(Some("running"))).or_else(|| newest(None));
    let all = pr_url.filter(|u| u.starts_with("http")).map(|u| {
        let u = u.trim_end_matches('/');
        if u.starts_with("https://github.com/") { format!("{u}/checks") } else { u.to_string() }
    });
    failed.or(all).map(Value::String).unwrap_or(Value::Null)
}

/// `pr.bar` on a task card with a PR.
pub fn bar(app: &App, t: &Row) -> Result<Value> {
    let f = jloads_obj(t.s("pr_flow"));
    let rec = f.get("rec").cloned().unwrap_or(json!({}));
    let head = rec["head"].as_str().unwrap_or("");
    let skipped = prflow::checks_skipped(&f, &rec);
    let failing = !prflow::failing(&f, &rec).is_empty();
    let cleared = !prflow::not_ours(&f, &rec).is_null() && !failing && rec["failed"].as_array().is_some_and(|a| !a.is_empty());
    // Skipped (a hook, or `tb pr skip-checks` for builds that didn't fail) is told apart from failures
    // cleared as not this PR's, which come with proof.
    let checks = if cleared {
        "not_ours"
    } else if skipped.is_some() && (!failing || prflow::skip_hides_failures(&f, &rec)) {
        "skipped"
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
    let mut rows = reviewer_rows(&f, &rec);
    // Swaps and when each was asked come from the ask ledger (`asks.rs`).
    let info = crate::asks::pill_info(app, t.id())?;
    for r in rows.iter_mut() {
        if let Some((swaps, at)) = r["user"].as_str().and_then(|u| info.get(&u.to_lowercase())) {
            r["swaps"] = json!(swaps);
            r["asked_at"] = at.clone();
        }
    }
    // "x of N": the host's reviewer list when it has one, else approvals plus the reviewers still asked.
    let reviewers = if rec["reviewers"].is_array() { rows.len() as i64 } else { approvals + rec["requested"].as_i64().unwrap_or(0) };
    // The Review step: "off" when the project has none, "setup" when nobody's on the PR and the
    // board has no one it could ask (`tb reviewers add`, or `tb project set --review off`).
    let project = t.st("project");
    let review_step = if !crate::reviewers::review_on(app, Some(&project)) {
        "off"
    } else if reviewers == 0 && !crate::reviewers::any_askable(app, &project)? {
        "setup"
    } else {
        ""
    };
    let open = if rec["threads"].is_array() { prflow::open_threads(&f, &rec) } else { vec![] };
    let new_comments = if rec["threads"].is_array() { open.len() as i64 } else { (rec["comments"].as_i64().unwrap_or(0) - f.i0("comments_seen")).max(0) };
    // "N new comments" links to the first unread thread (the one waiting longest), else the PR.
    let comments_url = open
        .iter()
        .filter(|th| th["url"].as_str().is_some_and(|u| u.starts_with("http")))
        .min_by(|a, b| a["last_at"].as_str().unwrap_or("").cmp(b["last_at"].as_str().unwrap_or("")))
        .and_then(|th| th["url"].as_str().map(|u| u.to_string()))
        .or_else(|| t.s("pr_url").filter(|u| u.starts_with("http") && new_comments > 0).map(|u| u.to_string()));
    Ok(json!({
        "build_url": build_url(&rec, t.s("pr_url")),
        "review": if review_step.is_empty() { Value::Null } else { json!(review_step) },
        "checks": if checks.is_empty() { Value::Null } else { json!(checks) },
        "checks_why": skipped,
        "you": if you.is_empty() { Value::Null } else { json!(you) },
        "approvals": approvals, "reviewers": reviewers,
        "new_comments": new_comments,
        "comments_url": comments_url,
        "waits_on_base": t.s("pr_phase") == Some("waits"),
        "stacks_on": stack::card(app, t)?,
        "retargeted": f.get("retargeted").cloned().unwrap_or(Value::Null),
        "retarget_error": f.get("retarget_error").cloned().unwrap_or(Value::Null),
        "wd": crate::steps::bar_card(app, t)?,
        "reviewer_rows": rows,
    }))
}

/// One pill per reviewer still on the PR (swapped-off ones aside), from the host's reviewer states:
/// approved, changes (asked for changes), rereview (asked again after `tb pr addressed`), waiting
/// (asked, hasn't reviewed) or commented. `swaps` (how many swaps led to this reviewer) and
/// `asked_at` are filled in by `bar` from the ask ledger.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(checks: Value) -> Value {
        json!({"checks": checks})
    }

    #[test]
    fn checks_open_the_failed_build_else_the_latest_else_the_pr() {
        let bb = Some("https://bitbucket.org/ws/repo/pull-requests/7");
        let failed = rec(json!([{"name": "build", "state": "passed", "url": "https://ci/1"},
                                {"name": "test", "state": "failed", "url": "https://ci/2"},
                                {"name": "e2e", "state": "running", "url": "https://ci/3"}]));
        assert_eq!(build_url(&failed, bb), "https://ci/2", "a failed check opens itself");
        let running = rec(json!([{"name": "build", "state": "passed", "url": "https://ci/1"},
                                 {"name": "e2e", "state": "running", "url": "https://ci/3"},
                                 {"name": "lint", "state": "passed", "url": "https://ci/4"}]));
        assert_eq!(build_url(&running, bb), "https://ci/3", "a running build is the latest");
        let passed = rec(json!([{"name": "build", "state": "passed", "url": "https://ci/1"},
                                {"name": "lint", "state": "passed", "url": "https://ci/4"},
                                {"name": "gate", "state": "passed"}]));
        assert_eq!(build_url(&passed, bb), "https://ci/4", "else the last the host lists with a link");
        assert_eq!(build_url(&rec(json!([])), bb), bb.unwrap(), "no checks: the PR");
        assert_eq!(build_url(&rec(json!([{"name": "gate", "state": "passed"}])), bb), bb.unwrap(), "none with a link: the PR");
        assert_eq!(build_url(&rec(json!([])), Some("https://github.com/acme/web/pull/3/")), "https://github.com/acme/web/pull/3/checks");
        assert_eq!(build_url(&rec(json!([])), None), Value::Null);
    }

    #[test]
    fn checks_open_the_newest_build_by_its_time_not_the_hosts_order() {
        let bb = Some("https://bitbucket.org/ws/repo/pull-requests/7");
        // Bitbucket lists statuses unsorted: a re-run can come before the build it replaced.
        let running = rec(json!([{"name": "e2e #2", "state": "running", "url": "https://ci/new", "at": "2026-09-01T11:00:00.000000+00:00"},
                                 {"name": "e2e #1", "state": "running", "url": "https://ci/old", "at": "2026-09-01T10:00:00.000000+00:00"},
                                 {"name": "lint", "state": "passed", "url": "https://ci/lint", "at": "2026-09-01T12:00:00+00:00"}]));
        assert_eq!(build_url(&running, bb), "https://ci/new", "the newest running build, over a newer finished one");
        let passed = rec(json!([{"name": "build #9", "state": "passed", "url": "https://ci/9", "at": "2026-09-01T12:00:00Z"},
                                {"name": "build #8", "state": "stopped", "url": "https://ci/8", "at": "2026-09-01T09:00:00Z"},
                                {"name": "gate", "state": "passed", "url": "https://ci/gate"}]));
        assert_eq!(build_url(&passed, bb), "https://ci/9", "the newest build; one with no time counts as older");
        let failed = rec(json!([{"name": "build #9", "state": "passed", "url": "https://ci/9", "at": "2026-09-01T12:00:00Z"},
                                {"name": "test", "state": "failed", "url": "https://ci/test", "at": "2026-09-01T08:00:00Z"}]));
        assert_eq!(build_url(&failed, bb), "https://ci/test", "a failed check still comes first");
    }

    #[test]
    fn checks_open_the_newest_of_several_failed_builds() {
        let bb = Some("https://bitbucket.org/ws/repo/pull-requests/7");
        // A re-run that failed again (Bitbucket keys build-1 and build-2), listed older first or not.
        let rerun = rec(json!([{"name": "build-2", "state": "failed", "url": "https://ci/2", "at": "2026-09-01T11:00:00Z"},
                               {"name": "build-1", "state": "failed", "url": "https://ci/1", "at": "2026-09-01T10:00:00Z"}]));
        assert_eq!(build_url(&rerun, bb), "https://ci/2", "the newer failed build, listed first");
        let rerun = rec(json!([{"name": "build-1", "state": "failed", "url": "https://ci/1", "at": "2026-09-01T10:00:00Z"},
                               {"name": "build-2", "state": "failed", "url": "https://ci/2", "at": "2026-09-01T11:00:00Z"},
                               {"name": "e2e", "state": "running", "url": "https://ci/e2e", "at": "2026-09-01T12:00:00Z"}]));
        assert_eq!(build_url(&rerun, bb), "https://ci/2", "the newer failed build, over a newer running one");
        let untimed = rec(json!([{"name": "build-2", "state": "failed", "url": "https://ci/2", "at": "2026-09-01T11:00:00Z"},
                                 {"name": "gate", "state": "failed", "url": "https://ci/gate"}]));
        assert_eq!(build_url(&untimed, bb), "https://ci/2", "a failed check with no time counts as older");
        let ties = rec(json!([{"name": "a", "state": "failed", "url": "https://ci/a", "at": "2026-09-01T11:00:00Z"},
                              {"name": "b", "state": "failed", "url": "https://ci/b", "at": "2026-09-01T11:00:00+00:00"},
                              {"name": "c", "state": "failed", "url": "https://ci/c"},
                              {"name": "d", "state": "failed", "url": "https://ci/d"}]));
        assert_eq!(build_url(&ties, bb), "https://ci/b", "equal times: the one listed later");
        let none_timed = rec(json!([{"name": "c", "state": "failed", "url": "https://ci/c"},
                                    {"name": "d", "state": "failed", "url": "https://ci/d"},
                                    {"name": "e", "state": "failed"}]));
        assert_eq!(build_url(&none_timed, bb), "https://ci/d", "no times: the last failed one with a link");
    }
}
