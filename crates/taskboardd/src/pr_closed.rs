//! Where a PR closed without merging sent its work (#1098), kept in its flow as `closed`, so the PR card
//! and `tb` can tell "superseded" from "dropped" at a glance.
//!
//! `tb pr close` takes a reason or the PR or commit that replaced it, posts it on the PR and records it.
//! A PR closed outside the board (on its host, or with a raw `gh pr close`) is recorded with the comment
//! posted with the close, if there is one. With none, the board checks the branch against its base with
//! `git cherry`: when every commit is already there, it comments on the PR naming the base commits that
//! carry them; when not, it tells the owner the PR closed unmerged with its work not on the base.

use serde_json::{json, Value};

use crate::app::App;
use crate::prhost::PrRef;
use crate::util::*;
use crate::worktrees::{git, last_line, GIT_SECS};
use crate::{board, dispatch, prflow};

/// The flow key the record lives under.
pub const KEY: &str = "closed";
const FETCH_SECS: f64 = 300.0;
/// How many of the base's commits the board's comment names.
const NAMED: usize = 10;

fn alert_key(id: i64) -> String {
    format!("pr-closed:{id}")
}

/// What `tb pr close --superseded-by` names, for the comment and the card: `#12` for a PR in the same
/// repo, `owner/repo#12` for another, or a commit id.
pub fn superseder(given: &str, repo: &str) -> Result<String> {
    let g = given.trim();
    if let Some(link) = find_pr(g) {
        return Ok(if link.repo.eq_ignore_ascii_case(repo) { format!("#{}", link.num) } else { format!("{}#{}", link.repo, link.num) });
    }
    let num = g.strip_prefix('#').unwrap_or(g);
    if !num.is_empty() && num.len() < 7 && num.chars().all(|c| c.is_ascii_digit()) {
        return Ok(format!("#{num}"));
    }
    if (7..=40).contains(&g.len()) && g.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(g.to_lowercase());
    }
    err(400, format!("--superseded-by takes the PR that replaced it (#12 or its link) or the commit its work landed in, not “{}”.", one_line(g, 80)))
}

/// The comment `tb pr close` posts on the PR.
pub fn comment(reason: &str, superseded_by: &str) -> String {
    if superseded_by.is_empty() {
        format!("Closed without merging: {reason}")
    } else if reason.is_empty() {
        format!("Closed without merging: superseded by {superseded_by}.")
    } else {
        format!("Closed without merging: superseded by {superseded_by}. {reason}")
    }
}

/// Records that the board closed the PR (`tb pr close`), before it asks the host to, so a poll that
/// reads it closed first doesn't take the close for one made outside the board.
pub fn record_close(app: &App, id: i64, who: &str, reason: &str, superseded_by: &str) -> Result<()> {
    let mut c = json!({"how": "tb", "by": who, "at": now_iso()});
    if !reason.is_empty() {
        c["reason"] = json!(reason);
    }
    if !superseded_by.is_empty() {
        c["superseded_by"] = json!(superseded_by);
    }
    prflow::merge_flow(app, id, vec![(KEY, c)]).map(|_| ())
}

/// Takes back [`record_close`] when the host refused the close.
pub fn forget(app: &App, id: i64) -> Result<()> {
    prflow::merge_flow(app, id, vec![(KEY, Value::Null)]).map(|_| ())
}

/// The card's `closed`: what it said and one line for it; null while the board hasn't settled it.
pub fn card(flow: &Row) -> Value {
    let Some(c) = flow.get(KEY).filter(|c| c.is_object()) else { return Value::Null };
    let s = |k: &str| c[k].as_str().filter(|v| !v.is_empty());
    let base = s("base").unwrap_or("its base");
    let line = if let Some(by) = s("superseded_by") {
        match s("reason") {
            Some(r) => format!("Superseded by {by}: {r}"),
            None => format!("Superseded by {by}"),
        }
    } else if c["on_base"] == true {
        let named: Vec<&str> = c["commits"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).filter_map(|v| v.split(' ').next()).collect()).unwrap_or_default();
        if named.is_empty() { format!("Its work is on {base}") } else { format!("Its work is on {base} in {}", named.join(", ")) }
    } else if c["lost"] == true {
        format!("Closed unmerged; its work is not on {base}")
    } else if let Some(e) = s("unknown") {
        format!("Closed unmerged; couldn't tell whether its work is on {base} ({e})")
    } else if let Some(r) = s("reason") {
        format!("Closed: {r}")
    } else {
        return Value::Null;
    };
    json!({"line": one_line(&line, 300), "reason": c["reason"], "superseded_by": c["superseded_by"], "on_base": c["on_base"] == true, "lost": c["lost"] == true})
}

/// The comment posted with a close outside the board: the newest plain comment written since the board
/// last read the PR open (`since`, its last read's time).
fn closing_comment<'a>(rec: &'a Value, since: Option<&str>) -> Option<&'a Value> {
    let since = since.and_then(parse_iso)?;
    let at = |t: &Value| t["last_at"].as_str().and_then(parse_iso).unwrap_or(0.0);
    rec["threads"].as_array()?.iter().filter(|t| t["kind"] == "comment" && at(t) > since).max_by(|a, b| at(a).total_cmp(&at(b)))
}

/// The poll saw the PR closed unmerged. A close the board made is recorded already; one made outside
/// it takes the comment posted with it, else the board looks for its work on the base (off the database).
/// `f` is the flow before this read.
pub fn closed_outside(app: &App, t: &Row, f: &Row, rec: &Value) -> Result<()> {
    if f.get(KEY).is_some_and(|c| c.is_object()) {
        return Ok(());
    }
    let num = t.i0("pr_num");
    if let Some(c) = closing_comment(rec, f.s("checked_at")) {
        let last = c["replies"].as_array().and_then(|r| r.last());
        let text = last.and_then(|r| r["body"].as_str().filter(|b| !b.is_empty()).or(r["text"].as_str())).or(c["body"].as_str().filter(|b| !b.is_empty())).or(c["text"].as_str());
        let who = last.and_then(|r| r["author_name"].as_str()).or(c["author_name"].as_str()).unwrap_or("");
        let reason = one_line(text.unwrap_or(""), 1000);
        if !reason.is_empty() {
            prflow::merge_flow(app, t.id(), vec![(KEY, json!({"how": "host", "by": who, "at": now_iso(), "reason": reason}))])?;
            return Ok(());
        }
    }
    prflow::merge_flow(app, t.id(), vec![(KEY, json!({"how": "checking", "at": now_iso()}))])?;
    let id = t.id();
    app.defer(Box::new(move |app: &App| {
        if let Err(e) = check(app, id) {
            app.info(format!("pr closed: {} PR #{num}: {}", rf("task", id), e.message));
        }
    }));
    Ok(())
}

/// Where the branch's work is, against the base.
#[derive(Debug, PartialEq)]
pub enum OnBase {
    /// Every commit is on the base: the base's commits that carry them ("abc1234 subject"), newest first.
    Yes(Vec<String>),
    No,
}

/// Fetches `base` and the PR's head (`head`, else `branch`) into `repo` and compares them with
/// `git cherry`. An error says why it couldn't tell.
pub fn on_base(repo: &str, base: &str, branch: &str, head: &str) -> std::result::Result<OnBase, String> {
    let run = |args: &[&str], secs: f64| {
        let g = git(repo, args, secs);
        if g.code == 0 { Ok(g.out) } else { Err(last_line(&g.err)) }
    };
    let known = |r: &str| !r.is_empty() && git(repo, &["rev-parse", "--verify", "--quiet", &format!("{r}^{{commit}}")], GIT_SECS).code == 0;
    let base_ref = format!("refs/remotes/origin/{base}");
    if run(&["fetch", "-q", "--no-tags", "origin", &format!("+refs/heads/{base}:{base_ref}")], FETCH_SECS).is_err() && !known(&base_ref) {
        return Err(format!("couldn't fetch {base}"));
    }
    let branch_ref = format!("refs/remotes/origin/{branch}");
    if !known(head) && !branch.is_empty() {
        let _ = run(&["fetch", "-q", "--no-tags", "origin", &format!("+refs/heads/{branch}:{branch_ref}")], FETCH_SECS);
    }
    let tip = [head, branch_ref.as_str(), branch].into_iter().find(|r| known(r)).ok_or_else(|| format!("its branch {branch} isn't on origin or here"))?;
    let ours = run(&["cherry", &base_ref, tip], GIT_SECS)?;
    if ours.lines().any(|l| l.starts_with('+')) {
        return Ok(OnBase::No);
    }
    let fork = run(&["merge-base", &base_ref, tip], GIT_SECS)?;
    let theirs = run(&["cherry", "-v", tip, &base_ref, fork.trim()], GIT_SECS)?;
    let mut named: Vec<String> = theirs
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .map(|l| match l.split_once(' ') {
            Some((sha, subject)) => format!("{} {}", &sha[..sha.len().min(7)], one_line(subject, 80)),
            None => l[..l.len().min(7)].to_string(),
        })
        .collect();
    if named.is_empty() {
        let tip_sha = run(&["rev-parse", "--short=7", tip], GIT_SECS)?;
        named.push(tip_sha.trim().to_string());
    }
    named.reverse();
    Ok(OnBase::Yes(named))
}

/// Settles a PR closed outside the board with no comment: comments where its work went when it's on
/// the base, else tells the owner. Runs git and the host, so it stays out of transactions.
pub fn check(app: &App, id: i64) -> Result<()> {
    let t = board::get_task(app, id)?;
    let f = prflow::flow(&t);
    if f.get(KEY).and_then(|c| c["how"].as_str()) != Some("checking") {
        return Ok(());
    }
    let Some(pr) = PrRef::of(&t) else { return Ok(()) };
    let rec = f.get("rec").cloned().unwrap_or(Value::Null);
    let base = rec["base"].as_str().filter(|b| !b.is_empty()).unwrap_or("main").to_string();
    let (branch, head) = (rec["branch"].as_str().unwrap_or(""), rec["head"].as_str().unwrap_or(""));
    let found = match crate::worktrees::repo_of(app, &t)?.filter(|r| std::path::Path::new(r).is_dir()) {
        Some(repo) => on_base(&repo, &base, branch, head),
        None => Err("the board doesn't know where its repository is".into()),
    };
    let me = rf("task", id);
    let num = pr.num;
    let mut c = json!({"how": "board", "at": now_iso(), "base": base});
    match found {
        Ok(OnBase::Yes(commits)) => {
            let shown: Vec<&String> = commits.iter().take(NAMED).collect();
            let more = commits.len().saturating_sub(NAMED);
            let list = shown.iter().map(|c| format!("- {c}")).collect::<Vec<_>>().join("\n");
            let more = if more > 0 { format!("\n- and {more} more") } else { String::new() };
            let body = format!("Closed without merging, but its work is already on `{base}`, in:\n\n{list}{more}");
            let posted = crate::prhost::host_for(app, &pr.host).and_then(|h| h.comment(&pr, &body));
            c["on_base"] = json!(true);
            c["commits"] = json!(commits);
            app.db.tx(|| {
                prflow::merge_flow(app, id, vec![(KEY, c.clone())])?;
                let named = shown.iter().filter_map(|c| c.split(' ').next()).collect::<Vec<_>>().join(", ");
                let said = match &posted {
                    Ok(()) => "said so on the PR".to_string(),
                    Err(e) => format!("couldn't say so on the PR: {e}"),
                };
                board::log_event(app, id, board::BOARD, "status", &format!("PR #{num} was closed with no comment; its work is on {base} in {named}, and the board {said}")).map(|_| ())
            })
        }
        Ok(OnBase::No) => {
            c["lost"] = json!(true);
            let text = format!(
                "{me}'s PR #{num} closed unmerged; its work is not on {base}. If it landed another way: tb task set {me} --landed <commit>; \
                 or reopen the PR on its host."
            );
            flag(app, &t, c, &text)
        }
        Err(e) => {
            c["unknown"] = json!(one_line(&e, 200));
            let text = format!("{me}'s PR #{num} closed unmerged, and the board couldn't tell whether its work is on {base}: {e}.");
            flag(app, &t, c, &text)
        }
    }
}

fn flag(app: &App, t: &Row, c: Value, text: &str) -> Result<()> {
    app.db.tx(|| {
        prflow::merge_flow(app, t.id(), vec![(KEY, c.clone())])?;
        board::log_event(app, t.id(), board::BOARD, "status", text)?;
        dispatch::add_alert_keyed(app, text, Some(t.id()), t.i("goal_id"), &alert_key(t.id()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superseded_by_names_a_pr_or_a_commit() {
        assert_eq!(superseder("#12", "acme/webapp").unwrap(), "#12");
        assert_eq!(superseder("12", "acme/webapp").unwrap(), "#12");
        assert_eq!(superseder("https://github.com/acme/webapp/pull/12", "acme/webapp").unwrap(), "#12");
        assert_eq!(superseder("https://github.com/acme/api/pull/3", "acme/webapp").unwrap(), "acme/api#3");
        assert_eq!(superseder("ABCDEF1234", "acme/webapp").unwrap(), "abcdef1234");
        assert!(superseder("the other one", "acme/webapp").unwrap_err().message.contains("--superseded-by takes"));
    }

    #[test]
    fn the_comment_says_what_replaced_it_and_why() {
        assert_eq!(comment("Went another way", ""), "Closed without merging: Went another way");
        assert_eq!(comment("", "#12"), "Closed without merging: superseded by #12.");
        assert_eq!(comment("Smaller", "#12"), "Closed without merging: superseded by #12. Smaller");
    }

    #[test]
    fn the_card_line_tells_superseded_from_dropped() {
        let line = |c: Value| card(&jloads_obj(Some(&json!({"closed": c}).to_string())))["line"].clone();
        assert_eq!(line(json!({"how": "tb", "superseded_by": "#12"})), "Superseded by #12");
        assert_eq!(line(json!({"how": "tb", "reason": "Not needed"})), "Closed: Not needed");
        assert_eq!(line(json!({"how": "board", "base": "main", "on_base": true, "commits": ["abc1234 Fix it", "def5678 Test it"]})), "Its work is on main in abc1234, def5678");
        assert_eq!(line(json!({"how": "board", "base": "main", "lost": true})), "Closed unmerged; its work is not on main");
        assert_eq!(line(json!({"how": "checking"})), Value::Null, "nothing to show while the board looks");
    }

    #[test]
    fn a_closing_comment_is_one_written_since_the_last_open_read() {
        let rec = json!({"threads": [
            {"kind": "comment", "last_at": "2026-11-01T10:00:00Z", "text": "old"},
            {"kind": "review", "last_at": "2026-11-02T10:00:00Z", "text": "code"},
            {"kind": "comment", "last_at": "2026-11-02T11:00:00Z", "text": "Done in #14"},
        ]});
        assert_eq!(closing_comment(&rec, Some("2026-11-02T00:00:00Z")).unwrap()["text"], "Done in #14");
        assert!(closing_comment(&rec, Some("2026-11-03T00:00:00Z")).is_none());
        assert!(closing_comment(&rec, None).is_none(), "without a last read nothing counts");
    }
}
