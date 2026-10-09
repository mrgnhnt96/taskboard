//! `tb done --pr-body FILE`: the board checks the PR description (`[pr_body]` in config.toml) and the
//! branch (pushed, rebased on the remote base, no merge commits), then opens the PR itself: the title
//! starts with the task's ticket key, a `## Context` section is added, and the PR goes into the real
//! base (a stacked task's parent branch). The PR is opened on GitHub or Bitbucket Cloud through `prhost`
//! (`host` here picks it from the checkout's remote).

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::app::App;
use crate::config::PrBodyConfig;
use crate::util::*;
use crate::{board, p, stack, steps};

static BOARD_REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[TGB]\d+\b").unwrap());
static BOARD_WORD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\btask[ -]?board\b").unwrap());
static HEADING_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^#{1,6}\s+(.+?)\s*#*\s*$").unwrap());
static BULLET_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*(?:[-*+]|\d+[.)])\s+\S").unwrap());

/// What's wrong with a PR description, one line each; empty when it's fine.
pub fn body_problems(cfg: &PrBodyConfig, body: &str) -> Vec<String> {
    let mut out = vec![];
    if body.trim().is_empty() {
        return vec!["The description is empty.".into()];
    }
    // Sections: `## Name`, in order, outside code fences.
    let mut sections: Vec<(String, Vec<String>)> = vec![];
    let mut fenced = false;
    let mut prose: Vec<String> = vec![];
    for line in body.lines() {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        prose.push(line.to_string());
        if let Some(c) = HEADING_RE.captures(line.trim_end()) {
            sections.push((c[1].trim().to_string(), vec![]));
            continue;
        }
        if let Some(s) = sections.last_mut() {
            s.1.push(line.to_string());
        }
    }
    let found = |name: &str| sections.iter().position(|(n, _)| n.eq_ignore_ascii_case(name));
    let mut last = None;
    for want in &cfg.sections {
        match found(want) {
            None => out.push(format!("It needs a “## {want}” section.")),
            Some(i) => {
                if last.map(|l| i < l).unwrap_or(false) {
                    out.push(format!("The sections go in this order: {}.", cfg.sections.iter().map(|s| format!("## {s}")).collect::<Vec<_>>().join(", ")));
                }
                last = Some(i);
            }
        }
    }
    for want in &cfg.bullets {
        let Some(i) = found(want) else { continue };
        let lines: Vec<&String> = sections[i].1.iter().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            out.push(format!("“## {want}” is empty."));
        } else if lines.iter().any(|l| !BULLET_RE.is_match(l) && !l.starts_with(' ') && !l.starts_with('\t')) {
            out.push(format!("“## {want}” should be a bullet list (- …), one change per line."));
        }
    }
    if cfg.max_paragraph > 0 {
        for p in paragraphs(&prose) {
            let n = p.chars().count();
            if n > cfg.max_paragraph {
                out.push(format!("A paragraph is {n} characters (“{}…”); keep each to {} or fewer.", p.chars().take(40).collect::<String>(), cfg.max_paragraph));
            }
        }
    }
    let text = prose.join("\n");
    if cfg.no_board_refs {
        if let Some(m) = BOARD_REF_RE.find(&text) {
            out.push(format!("It mentions {}: the board's task, goal and backlog refs mean nothing to a reviewer; say what the work is instead.", m.as_str()));
        }
        if let Some(m) = BOARD_WORD_RE.find(&text) {
            out.push(format!("It mentions “{}”: leave the board out of the PR.", m.as_str()));
        }
    }
    for pat in &cfg.forbid {
        match Regex::new(&format!("(?i){pat}")) {
            Ok(re) => {
                if let Some(m) = re.find(&text) {
                    out.push(format!("It says “{}”, which PR descriptions here leave out.", m.as_str()));
                }
            }
            Err(_) => out.push(format!("The [pr_body] forbid pattern “{pat}” isn't a regex; fix it in config.toml.")),
        }
    }
    out
}

/// The prose paragraphs (not headings, bullets or blank lines), each joined into one line.
fn paragraphs(lines: &[String]) -> Vec<String> {
    let mut out = vec![];
    let mut cur: Vec<&str> = vec![];
    for l in lines {
        let t = l.trim();
        let breaks = t.is_empty() || HEADING_RE.is_match(t) || BULLET_RE.is_match(l) || t.starts_with('>') || t.starts_with('|');
        if breaks {
            if !cur.is_empty() {
                out.push(cur.join(" "));
                cur.clear();
            }
            if BULLET_RE.is_match(l) {
                out.push(t.to_string());
            }
            continue;
        }
        cur.push(t);
    }
    if !cur.is_empty() {
        out.push(cur.join(" "));
    }
    out
}

/// The PR's title: the task's (or the one given), after its ticket key when there is one.
pub fn title(cfg: &PrBodyConfig, t: &Row, given: &str) -> String {
    let base = if given.trim().is_empty() { t.st("title") } else { one_line(given, 200) };
    match t.s("jira_key").filter(|k| !k.is_empty() && cfg.title_prefix) {
        Some(k) if !base.to_uppercase().starts_with(&k.to_uppercase()) => format!("{k} {base}"),
        _ => base,
    }
}

/// The `## Context` section: the ticket, the PR it stacks on, the task's evidence (web links, and local
/// files by name), and its design links under **Design**.
pub fn context_block(app: &App, t: &Row) -> Result<String> {
    let mut lines = vec![];
    if let Some(k) = t.s("jira_key").filter(|k| !k.is_empty()) {
        match board::jira_url(app, k).as_str() {
            Some(u) => lines.push(format!("- Ticket: [{k}]({u})")),
            None => lines.push(format!("- Ticket: {k}")),
        }
    }
    if let Some(p) = stack::parent(app, t)? {
        match (p.i("pr_num"), p.s("pr_url").filter(|u| !u.is_empty())) {
            (Some(n), Some(u)) if !stack::merged(&p) => lines.push(format!("- Stacks on #{n} ({u}): merge that first")),
            _ => {}
        }
    }
    let atts = board::attachments(app, Some(t.id()), None)?;
    let of = |kinds: &[&str]| -> Vec<(String, String)> {
        atts.iter()
            .filter(|a| a["kind"].as_str().is_some_and(|k| kinds.contains(&k)))
            .filter_map(|a| {
                let url = a["url"].as_str().filter(|u| !u.trim().is_empty())?;
                Some((a["title"].as_str().unwrap_or("").to_string(), url.trim().to_string()))
            })
            .collect()
    };
    // Evidence and results: a web link by its title, a local file by its name (a reviewer can't open it).
    for (title, url) in of(&["evidence", "results"]) {
        lines.push(if is_web(&url) { evidence_line(if title.is_empty() { "evidence" } else { &title }, &url) } else { format!("- Evidence: {}", file_name(&url)) });
    }
    let designs: Vec<String> = of(&["design"])
        .into_iter()
        .map(|(title, url)| if is_web(&url) { format!("- [{}]({url})", if title.is_empty() { "Design" } else { &title }) } else { format!("- {}", file_name(&url)) })
        .collect();
    if !designs.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("**Design**".into());
        lines.extend(designs);
    }
    Ok(if lines.is_empty() { String::new() } else { format!("## Context\n{}", lines.join("\n")) })
}

fn is_web(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// A local attachment's file name (`~/shots/login.png` → `login.png`).
fn file_name(path: &str) -> String {
    Path::new(path.trim_start_matches("file://")).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

/// The task's evidence and results that a reviewer can open (web links): (title, url).
pub fn evidence_links(app: &App, t: &Row) -> Result<Vec<(String, String)>> {
    Ok(board::attachments(app, Some(t.id()), None)?
        .iter()
        .filter(|a| a["kind"] == "evidence" || a["kind"] == "results")
        .filter_map(|a| {
            let url = a["url"].as_str().unwrap_or("");
            is_web(url).then(|| (a["title"].as_str().unwrap_or("evidence").to_string(), url.to_string()))
        })
        .collect())
}

fn evidence_line(title: &str, url: &str) -> String {
    format!("- Evidence: [{title}]({url})")
}

/// A PR description with the evidence links it doesn't have yet, under its `## Context` section (made
/// at the end when there's none); None when it has them all.
pub fn with_evidence(body: &str, links: &[(String, String)]) -> Option<String> {
    let new: Vec<String> = links.iter().filter(|(_, u)| !body.contains(u.as_str())).map(|(t, u)| evidence_line(t, u)).collect();
    if new.is_empty() {
        return None;
    }
    let lines: Vec<&str> = body.trim_end().lines().collect();
    let ctx = lines.iter().position(|l| HEADING_RE.captures(l.trim_end()).map(|c| c[1].eq_ignore_ascii_case("context")).unwrap_or(false));
    let Some(start) = ctx else {
        let sep = if body.trim().is_empty() { "" } else { "\n\n" };
        return Some(format!("{}{sep}## Context\n{}", body.trim_end(), new.join("\n")));
    };
    let end = lines[start + 1..].iter().position(|l| HEADING_RE.is_match(l.trim_end())).map(|i| start + 1 + i).unwrap_or(lines.len());
    let last = lines[start..end].iter().rposition(|l| !l.trim().is_empty()).map(|i| start + i + 1).unwrap_or(start + 1);
    let mut out: Vec<String> = lines[..last].iter().map(|l| l.to_string()).collect();
    out.extend(new);
    out.extend(lines[last..].iter().map(|l| l.to_string()));
    Some(out.join("\n"))
}

/// Minutes before trying again to add evidence to a PR whose host said no.
const EVIDENCE_RETRY_MINS: f64 = 30.0;

/// Adds each open PR's task evidence to its description once the PR is open, as the Python board did
/// (`pr_flow.evidence_added` keeps the links added; a link already in the description isn't added
/// again). Outside any transaction (it calls the host). Returns how many PRs changed.
pub fn add_evidence(app: &App) -> Result<i64> {
    let rows = app.db.q(
        "SELECT * FROM tasks WHERE pr_host IN ('github', 'bitbucket') AND pr_num IS NOT NULL \
         AND (pr_phase IS NULL OR pr_phase NOT IN ('merged', 'declined'))",
        p![],
    )?;
    let mut changed = 0;
    for t in rows {
        let f = jloads_obj(t.s("pr_flow"));
        if f.s("evidence_retry_at").map(|r| r > now_iso().as_str()).unwrap_or(false) {
            continue;
        }
        let done: Vec<String> = f.get("evidence_added").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
        let links: Vec<(String, String)> = evidence_links(app, &t)?.into_iter().filter(|(_, u)| !done.contains(u)).collect();
        if links.is_empty() {
            continue;
        }
        let Some(pr) = crate::prhost::PrRef::of(&t) else { continue };
        let res = crate::prhost::host_for(app, &pr.host).and_then(|h| {
            let body = h.description(&pr)?;
            match with_evidence(&body, &links) {
                Some(next) => h.set_description(&pr, &next).map(|_| true),
                None => Ok(false),
            }
        });
        let all: Vec<String> = done.iter().cloned().chain(links.iter().map(|(_, u)| u.clone())).collect();
        app.db.tx(|| {
            match &res {
                Ok(edited) => {
                    crate::prflow::merge_flow(app, t.id(), crate::fields!["evidence_added" => all.clone(), "evidence_retry_at" => null])?;
                    if *edited {
                        let what = links.iter().map(|(title, _)| title.as_str()).collect::<Vec<_>>().join(", ");
                        board::log_event(app, t.id(), board::BOARD, "status", &format!("Added the evidence to PR #{}: {what}", pr.num))?;
                    }
                }
                Err(e) => {
                    crate::prflow::merge_flow(app, t.id(), crate::fields!["evidence_retry_at" => iso(now_ts() + EVIDENCE_RETRY_MINS * 60.0)])?;
                    app.info(format!("prs: couldn't add evidence to {}: {e}", t.st("pr_url")));
                }
            }
            Ok(())
        })?;
        if matches!(res, Ok(true)) {
            changed += 1;
        }
    }
    Ok(changed)
}

/// Runs git in `dir`: its exit and trimmed stdout, or why it couldn't run.
fn git(dir: &str, args: &[&str], timeout: f64) -> std::result::Result<(bool, String, String), String> {
    let bin = crate::proc::which("git").ok_or("git isn't installed")?;
    let mut all: Vec<String> = vec!["-C".into(), dir.into()];
    all.extend(args.iter().map(|a| a.to_string()));
    match crate::proc::run(&bin, &all, None, timeout) {
        Ok(o) => Ok((o.code == Some(0), o.stdout.trim().to_string(), o.stderr.trim().to_string())),
        Err(_) => Err(format!("git {} didn't finish", args.first().unwrap_or(&""))),
    }
}

/// The branch, ready for a PR into `base`: checked out, pushed as it is, rebased on the remote base
/// with no merge commits. The branch's name, or why not.
pub fn check_branch(cfg: &PrBodyConfig, dir: &str, base: &str) -> std::result::Result<String, String> {
    let remote = &cfg.remote;
    let (ok, branch, _) = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"], 10.0)?;
    if !ok || branch.is_empty() || branch == "HEAD" {
        return Err("This checkout isn't on a branch: make the task's branch (git switch -c <branch>) and push it.".into());
    }
    if branch == base {
        return Err(format!("This is {base} itself: the PR needs the task's own branch (git switch -c <branch>)."));
    }
    let (ok, _, _) = git(dir, &["fetch", remote, base, &branch], 120.0)?;
    if !ok {
        let (ok, _, e) = git(dir, &["fetch", remote, base], 120.0)?;
        if !ok {
            // All of git's message: its last line alone ("and the repository exists.") says nothing.
            let e = e.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
            return Err(format!("git fetch {remote} {base} failed:\n{}", if e.is_empty() { "git said nothing" } else { &e }));
        }
    }
    let (_, head, _) = git(dir, &["rev-parse", "HEAD"], 10.0)?;
    let (pushed, there, _) = git(dir, &["rev-parse", "--verify", "-q", &format!("refs/remotes/{remote}/{branch}")], 10.0)?;
    if !pushed {
        return Err(format!("{branch} isn't pushed: git push -u {remote} {branch}, then run tb done again."));
    }
    if there != head {
        return Err(format!("{remote}/{branch} isn't this commit: push it (git push --force-with-lease after a rebase), then run tb done again."));
    }
    if cfg.rebased {
        let base_ref = format!("{remote}/{base}");
        let (on_top, _, _) = git(dir, &["merge-base", "--is-ancestor", &base_ref, "HEAD"], 10.0)?;
        if !on_top {
            return Err(format!("{branch} isn't rebased on {base_ref}: git rebase {base_ref}, push with --force-with-lease, then run tb done again."));
        }
        let (_, merges, _) = git(dir, &["rev-list", "--merges", &format!("{base_ref}..HEAD")], 10.0)?;
        if !merges.is_empty() {
            return Err(format!(
                "{branch} has {} on top of {base_ref}: rebase instead (git rebase {base_ref}), push with --force-with-lease, then run tb done again.",
                plural(merges.lines().count() as i64, "merge commit")
            ));
        }
    }
    Ok(branch)
}

/// The folder the PR is opened from: the report's checkout, else the task's worktree or repo.
fn dir_for(t: &Row, cwd: &str) -> Option<String> {
    let ctx = board::task_context(t);
    let wt = ctx.get("where").and_then(|w| w.get("worktree")).and_then(|v| v.as_str()).map(|s| s.to_string());
    [Some(cwd.to_string()), wt, t.s("repo_path").map(|s| s.to_string())].into_iter().flatten().find(|d| !d.is_empty() && Path::new(d).is_dir())
}

/// Opens the task's PR from `--pr-body`: the description checked, the branch checked, the PR opened on
/// the host. Its link, or a refusal for the agent.
pub fn open(app: &App, t: &Row, cwd: &str, body: &str, given_title: &str) -> Result<PrLink> {
    let cfg = &app.cfg.pr_body;
    let problems = body_problems(cfg, body);
    if !problems.is_empty() {
        return err(400, format!("The PR description needs work before the board opens the PR:\n- {}\nFix the file and run tb done again.", problems.join("\n- ")));
    }
    if cfg.require_ticket && !has(t.s("jira_key")) {
        return err(409, format!("{} has no Jira ticket, and PRs here need one: link it with tb task set {} --jira <KEY> (or new).", rf("task", t.id()), rf("task", t.id())));
    }
    let Some(dir) = dir_for(t, cwd) else { return err(409, "The board can't find this task's checkout to open the PR from.") };
    let base = steps::real_base(app, t)?;
    let branch = check_branch(cfg, &dir, &base).map_err(|e| ApiError::new(409, e))?;
    let mut text = body.trim_end().to_string();
    if cfg.context {
        let ctx = context_block(app, t)?;
        if !ctx.is_empty() {
            text += &format!("\n\n{ctx}");
        }
    }
    let title = title(cfg, t, given_title);
    host::open(app, &dir, &base, &branch, &title, &text).map_err(|e| ApiError::new(502, format!("Couldn't open the PR: {e}")))
}

/// Runs before `tb done` (outside its transaction): with `pr_body`, opens the PR and puts its link in
/// the report, so the rest of `tb done` links it. A task that already has its PR keeps it.
pub fn before_done(app: &App, body: &mut Value, t: Option<&Row>) -> Result<()> {
    let text = body_str(body, "pr_body");
    if text.trim().is_empty() {
        return Ok(());
    }
    let Some(t) = t.filter(|t| t.s("status") != Some("done")) else { return Ok(()) };
    if has(t.s("pr_url")) {
        body["pr"] = json!(t.st("pr_url"));
        return Ok(());
    }
    if !body_str(body, "pr").is_empty() {
        return err(400, "Give either --pr (a PR you opened) or --pr-body (the board opens it), not both.");
    }
    let left = steps::missing_at(app, t, &[steps::Before::Pr, steps::Before::Done], body["git"]["sha"].as_str().filter(|h| !h.is_empty()))?;
    if !left.is_empty() {
        return err(409, steps::refusal_for(app, t, "opening the PR", &left));
    }
    let pr = open(app, t, &body_str(body, "cwd"), &text, &body_str(body, "pr_title"))?;
    app.db.tx(|| {
        let t = board::get_task(app, t.id())?;
        crate::prflow::link_pr(app, &t, &pr, board::BOARD)?;
        board::log_event(app, t.id(), board::BOARD, "status", &format!("Opened PR #{} into {}", pr.num, steps::real_base(app, &t)?))
    })?;
    body["pr"] = json!(pr.url);
    Ok(())
}

/// The PR host, through `prhost`: GitHub or Bitbucket Cloud, by the checkout's remote.
pub mod host {
    use super::*;

    /// The host and repo of the checkout's remote (`[pr_body] remote`).
    fn remote(app: &App, dir: &str) -> std::result::Result<(String, String), String> {
        let r = &app.cfg.pr_body.remote;
        let (ok, url, _) = git(dir, &["remote", "get-url", r], 10.0)?;
        if !ok {
            return Err(format!("this checkout has no {r} remote"));
        }
        crate::prhost::repo_of_remote(&url).ok_or_else(|| {
            format!("{r} ({url}) isn't on GitHub or Bitbucket; open the PR with the host's tools and pass its link to tb done --pr")
        })
    }

    /// Opens a PR from `branch` into `base`; its link. A remote that names neither host (a mirror, an
    /// SSH alias) is left to `gh pr create` in the checkout, which knows the repo from its own config.
    pub fn open(app: &App, dir: &str, base: &str, branch: &str, title: &str, body: &str) -> std::result::Result<PrLink, String> {
        let (host, repo) = match remote(app, dir) {
            Ok(r) => r,
            Err(why) => return gh_create(app, dir, base, branch, title, body).map_err(|e| format!("{e} ({why})")),
        };
        let pr = crate::prhost::host_for(app, &host)?.open(&repo, base, branch, title, body)?;
        Ok(PrLink { host: pr.host, repo: pr.repo, num: pr.num, url: pr.url })
    }

    fn gh_create(app: &App, dir: &str, base: &str, branch: &str, title: &str, body: &str) -> std::result::Result<PrLink, String> {
        let gh = crate::proc::which(&app.cfg.pr.gh).ok_or_else(|| "the gh command isn't installed".to_string())?;
        let args: Vec<String> = ["pr", "create", "--base", base, "--head", branch, "--title", title, "--body-file", "-"].iter().map(|s| s.to_string()).collect();
        let o = crate::proc::run_with(&gh, &args, Some(Path::new(dir)), 60.0, &crate::accounts::gh_env(&app.cfg), Some(body.as_bytes()))
            .map_err(|_| "gh didn't answer in 60 seconds".to_string())?;
        if o.code != Some(0) {
            return Err(o.stderr.trim().lines().last().filter(|l| !l.is_empty()).unwrap_or("gh failed").to_string());
        }
        find_pr(&o.stdout).ok_or_else(|| format!("gh didn't say where the PR is: {}", one_line(&o.stdout, 200)))
    }

    /// Points an open PR at another base branch.
    pub fn retarget(app: &App, t: &Row, base: &str) -> std::result::Result<(), String> {
        let pr = crate::prhost::PrRef::of(t).ok_or_else(|| "the task has no PR".to_string())?;
        crate::prhost::host_for(app, &pr.host)?.retarget(&pr, base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "## Summary\nAdds passkey sign-in.\n\n## Changes\n- New sign-in screen\n- Server checks the key\n\n## Testing\n- Unit tests\n";

    #[test]
    fn a_good_body_passes() {
        assert!(body_problems(&PrBodyConfig::default(), GOOD).is_empty(), "{:?}", body_problems(&PrBodyConfig::default(), GOOD));
    }

    #[test]
    fn sections_bullets_length_and_refs() {
        let c = PrBodyConfig::default();
        let p = body_problems(&c, "## Changes\n- x\n## Summary\nY\n");
        assert!(p.iter().any(|x| x.contains("“## Testing”")), "{p:?}");
        assert!(p.iter().any(|x| x.contains("this order")), "{p:?}");
        let p = body_problems(&c, &GOOD.replace("- New sign-in screen", "A new screen"));
        assert!(p.iter().any(|x| x.contains("bullet list")), "{p:?}");
        let p = body_problems(&c, &GOOD.replace("Adds passkey sign-in.", &"word ".repeat(100)));
        assert!(p.iter().any(|x| x.contains("characters")), "{p:?}");
        let p = body_problems(&c, &GOOD.replace("Adds passkey sign-in.", "Done for T12 on the task board."));
        assert!(p.iter().any(|x| x.contains("T12")) && p.iter().any(|x| x.contains("task board")), "{p:?}");
        let c2 = PrBodyConfig { forbid: vec!["warp ?drive".into()], ..PrBodyConfig::default() };
        let p = body_problems(&c2, &GOOD.replace("Unit tests", "Reviewed by Warp Drive"));
        assert!(p.iter().any(|x| x.contains("Warp Drive")), "{p:?}");
        // Code isn't prose.
        let p = body_problems(&c, &format!("{GOOD}\n```\nT12 {}\n```\n", "x".repeat(500)));
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn the_title_starts_with_the_ticket() {
        let c = PrBodyConfig::default();
        let mut t = Row::new();
        t.insert("title".into(), json!("Add login"));
        assert_eq!(title(&c, &t, ""), "Add login");
        t.insert("jira_key".into(), json!("ABC-12"));
        assert_eq!(title(&c, &t, ""), "ABC-12 Add login");
        assert_eq!(title(&c, &t, "ABC-12: Sign in"), "ABC-12: Sign in");
        assert_eq!(title(&PrBodyConfig { title_prefix: false, ..c }, &t, ""), "Add login");
    }
}
