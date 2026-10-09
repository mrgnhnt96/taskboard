//! Reviewers: each project's roster of people who review its PRs, and the ledger of every time the
//! board (or an agent through `tb pr reviewers`) asked one of them.
//!
//! One row per person and project. Several commit emails, host accounts and spellings fold into
//! one row: the extra ones are its aliases, and any of them names the reviewer in `tb reviewers …`
//! and `tb pr reviewers --replace/--drop`. A removed reviewer is never asked again ("never assign
//! this person") until `tb reviewers back`; a pinned one comes first for the main-contributor pick. The
//! picker (`picker.rs`) chooses who to ask, the availability check (`presence.rs`) and bot schedules
//! feed it, and the sweep (`asks.rs`) times asks out and swaps reviewers.
//!
//! # Tables
//!
//! `reviewers`: the roster.
//!
//! | Column | |
//! |---|---|
//! | `id` | |
//! | `project` | the project's name (`tasks.project`) |
//! | `name` | how the board says them |
//! | `host_user` | the PR host's id: a GitHub login or a Bitbucket `{uuid}`; NULL until known (can't be asked yet) |
//! | `emails` | JSON array of commit emails |
//! | `aliases` | JSON array of other names, emails and host ids folded into this row |
//! | `slack` | their Slack user id or email, for the availability check (else their first email) |
//! | `source` | how they joined: `tb`, `git` (commit history), `host` (the repo's members), `import` |
//! | `commits` | their commits in the history window at the last sync |
//! | `removed_at`, `removed_why` | removed: never asked until `tb reviewers back` |
//! | `pinned` | 1: first in line for the main-contributor pick |
//! | `automation` | how much of their reviewing is automated (`tb reviewers auto`), a weight: 1 is normal |
//! | `bot_every_h`, `bot_mark` | their review bot runs about every this many hours and marks its comments with this text |
//! | `carried_asks`, `carried_swaps`, `carried_last_ask` | from the old board (`taskboardd import`): its asks and swaps of them that its ledger didn't keep, and its last ask; counted with the ledger's for their turn |
//! | `created_at`, `updated_at` | |
//!
//! `review_asks`: one row per ask of one reviewer on one PR.
//!
//! | Column | |
//! |---|---|
//! | `id` | |
//! | `task_id`, `project`, `pr_host`, `pr_repo`, `pr_num` | the PR |
//! | `reviewer_id` | the roster row (NULL for someone not on it) |
//! | `host_user`, `name` | who was asked, as the host knows them |
//! | `why` | `pick` (the picker), `ask` (named with `--ask`), `replace` (`--replace`), `swap` (timed out), `fill_in` (a swapped-off reviewer asked for changes), `stage` (the board's own ask at the `ask` stage), `rereview` (`tb pr addressed` asked them to look again) |
//! | `asked_by` | who asked: the agent, the owner, the board |
//! | `replaces` | the ask this one stands in for |
//! | `state` | `open` (no review yet), `answered`, `swapped` (timed out and replaced), `came_back` (swapped off, then reviewed anyway), `dropped` (taken off the PR, by the board or on the host), `closed` (the PR merged or closed first) |
//! | `asked_at`, `answered_at`, `closed_at` | ISO times |
//! | `answer` | approved, changes or commented (an ask from the old board may keep its answer's own words) |
//! | `work_mins` | work minutes from the ask to the review: the reviewer's speed |
//! | `filled` | 1: a swapped-off reviewer's request for changes already got a fill-in |
//!
//! `reviewer_bot_runs`: runs of a reviewer's bot spotted in PR comments (`reviewer_id`, `at`, and
//! `ref`, the comment that showed it; unique per reviewer).

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::util::*;
use crate::{board, fields, p};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS reviewers(
  id INTEGER PRIMARY KEY, project TEXT NOT NULL, name TEXT NOT NULL, host_user TEXT,
  emails TEXT DEFAULT '[]', aliases TEXT DEFAULT '[]', slack TEXT, source TEXT, commits INT DEFAULT 0,
  removed_at TEXT, removed_why TEXT, pinned INT DEFAULT 0, automation REAL DEFAULT 1,
  bot_every_h REAL, bot_mark TEXT, created_at TEXT, updated_at TEXT);
CREATE INDEX IF NOT EXISTS reviewers_project ON reviewers(project);
CREATE TABLE IF NOT EXISTS review_asks(
  id INTEGER PRIMARY KEY, task_id INT, project TEXT, pr_host TEXT, pr_repo TEXT, pr_num INT,
  reviewer_id INT, host_user TEXT, name TEXT, why TEXT, asked_by TEXT, replaces INT,
  state TEXT DEFAULT 'open', asked_at TEXT, answered_at TEXT, closed_at TEXT, answer TEXT, work_mins REAL,
  filled INT DEFAULT 0);
CREATE INDEX IF NOT EXISTS review_asks_task ON review_asks(task_id);
CREATE INDEX IF NOT EXISTS review_asks_reviewer ON review_asks(reviewer_id);
CREATE TABLE IF NOT EXISTS reviewer_bot_runs(
  reviewer_id INT NOT NULL, at TEXT NOT NULL, ref TEXT NOT NULL, PRIMARY KEY(reviewer_id, ref));
"#;

/// Columns added to the reviewer tables since they were made: (table, column, type).
pub const ADDED: &[(&str, &str, &str)] = &[
    ("reviewers", "carried_asks", "INT DEFAULT 0"),
    ("reviewers", "carried_swaps", "INT DEFAULT 0"),
    ("reviewers", "carried_last_ask", "TEXT"),
];

/// How many times they've been asked, the old board's asks included.
pub fn ask_count(app: &App, r: &Row) -> Result<i64> {
    Ok(app.db.count("SELECT COUNT(*) FROM review_asks WHERE reviewer_id = ?", p![r.id()])? + r.i0("carried_asks"))
}

/// When they were last asked (ISO), the old board's last ask included.
pub fn last_asked(app: &App, r: &Row) -> Result<Value> {
    let ledger = app.db.val("SELECT MAX(asked_at) FROM review_asks WHERE reviewer_id = ?", p![r.id()])?;
    let carried = r.v("carried_last_ask");
    let at = |v: &Value| v.as_str().and_then(parse_iso);
    Ok(match (at(&ledger), at(&carried)) {
        (Some(a), Some(b)) if b > a => carried,
        (None, Some(_)) => carried,
        _ => ledger,
    })
}

/// `[reviewers]` in config.toml: how the picker (`picker.rs`) chooses who to ask.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReviewersConfig {
    /// How many reviewers a PR gets.
    pub count: usize,
    /// How many of the changed files' top contributors the main-contributor pick comes from.
    pub main_contributors: usize,
    /// How far back commit history counts, in months.
    pub history_months: i64,
    /// Commits in that window to join the roster from the history.
    pub min_commits: i64,
    /// Each open ask pushes a reviewer's next turn back this many hours (divided by their weight).
    pub turn_gap_hours: f64,
    /// Speed by median review time: [[work minutes up to, speed], …], fastest first.
    pub speed_by_minutes: Vec<(f64, f64)>,
    /// The speed of someone slower than the table's last step (but under `slow_cap_mins`).
    pub slow_speed: f64,
    /// The speed of someone whose median is `slow_cap_mins` or more: mostly non-answers.
    pub too_slow: f64,
    /// The speed of someone who hasn't reviewed yet.
    pub no_speed_yet: f64,
    /// The owner's own names, emails and host ids: never picked (besides the PR's author).
    pub me: Vec<String>,
    /// How often the roster picks up commit authors and host accounts, in hours.
    pub sync_every_hours: f64,
    /// Where the picker checks who's around (`presence.rs`): "" (nowhere) or "slack".
    pub availability: String,
    /// How many candidates it checks per pick.
    pub pick_tries: usize,
    /// A Slack status matching this (a regex) is out: never picked.
    pub out_pattern: String,
    /// Each person's working hours in their own time zone; outside them they're off.
    pub local_start: String,
    pub local_end: String,
    pub weekends_off: bool,
    /// Someone Slack doesn't know is taken off the roster ("not on Slack").
    pub drop_not_on_slack: bool,
    /// How long one check holds, in minutes.
    pub cache_mins: f64,
    /// How far back a reviewer's marked comments count as their bot's runs, in hours.
    pub bot_window_hours: f64,
    /// How many of the repo's most recently updated PRs are read for bot runs, and how often, in minutes.
    pub bot_scan_prs: usize,
    pub bot_scan_mins: f64,
    /// Marked comments this close together are one run, in minutes.
    pub bot_run_gap_mins: f64,
    /// A timed bot's owner is asked only when it runs within this many minutes.
    pub bot_due_mins: f64,
    /// Replace a reviewer who hasn't reviewed within `swap_after_mins` work minutes (through the host).
    pub swap: bool,
    pub swap_after_mins: f64,
    /// After the owner's own review, the PR waits in the `ask` stage until reviewers are asked:
    /// the agent asks (`tb pr reviewers`), or outside work hours the board does.
    pub ask_stage: bool,
    /// Seconds between the board's tries when its own ask fails.
    pub ask_retry_waits: Vec<i64>,
    /// A reviewer's speed comes from their last `speed_asks` asks of the last `speed_days` days.
    pub speed_days: f64,
    pub speed_asks: usize,
    /// An ask swapped off for not reviewing counts as this many work minutes, and one still open as
    /// its time so far, up to this. A median this slow is `too_slow`.
    pub slow_cap_mins: f64,
    /// Away on Slack with no post today is quiet only from this time where they are; before it
    /// they're starting their day.
    pub quiet_from: String,
    /// An out status seen within this many hours still holds outside the work hours, when nobody's checked.
    pub out_keeps_hours: f64,
}

impl Default for ReviewersConfig {
    fn default() -> Self {
        ReviewersConfig {
            count: 2,
            main_contributors: 3,
            history_months: 6,
            min_commits: 5,
            turn_gap_hours: 4.0,
            speed_by_minutes: vec![(30.0, 3.0), (60.0, 2.0), (120.0, 1.5), (240.0, 1.0)],
            slow_speed: 0.5,
            too_slow: 0.05,
            no_speed_yet: 1.0,
            me: vec![],
            sync_every_hours: 12.0,
            availability: String::new(),
            pick_tries: 6,
            out_pattern: r"(?i)\b(out|ooo|off|vacation|holiday|pto|sick|leave)\b|🌴|🤒|🏖".into(),
            local_start: "08:00".into(),
            local_end: "18:00".into(),
            weekends_off: true,
            drop_not_on_slack: true,
            cache_mins: 10.0,
            bot_window_hours: 12.0,
            bot_scan_prs: 20,
            bot_scan_mins: 10.0,
            bot_run_gap_mins: 30.0,
            bot_due_mins: 45.0,
            swap: false,
            swap_after_mins: 90.0,
            ask_stage: false,
            ask_retry_waits: vec![60, 300, 900],
            speed_days: 14.0,
            speed_asks: 8,
            slow_cap_mins: 240.0,
            quiet_from: "10:00".into(),
            out_keeps_hours: 24.0,
        }
    }
}

/// Whether a project's PRs have the `ask` stage: its own switch (`tb project set --ask-stage`,
/// `[pr.projects.<name>] ask_stage`), else `[reviewers] ask_stage`.
pub fn ask_stage_on(app: &App, project: Option<&str>) -> bool {
    crate::projects::pr_rules(app, project).ask_stage.unwrap_or(app.cfg.reviewers.ask_stage)
}

/// Whether reviewers who take too long are swapped on a project's PRs: its own switch (`tb project
/// set --swap`, `[pr.projects.<name>] swap`), else `[reviewers] swap`.
pub fn swap_on(app: &App, project: Option<&str>) -> bool {
    crate::projects::pr_rules(app, project).swap.unwrap_or(app.cfg.reviewers.swap)
}

/// Automation levels by name (`tb reviewers auto <who> low|normal|high|<number>`).
pub const LEVELS: [(&str, f64); 4] = [("off", 0.25), ("low", 0.5), ("normal", 1.0), ("high", 2.0)];

fn list_of(r: &Row, k: &str) -> Vec<String> {
    jloads_arr(r.s(k)).into_iter().filter_map(|v| v.as_str().map(|s| s.to_string())).filter(|s| !s.trim().is_empty()).collect()
}

fn low(s: &str) -> String {
    s.trim().trim_start_matches('@').to_lowercase()
}

/// Everything that names this reviewer, lowercased: name, host id, emails, aliases.
pub fn idents(r: &Row) -> Vec<String> {
    let mut out = vec![low(&r.st("name"))];
    if let Some(u) = r.s("host_user").filter(|u| !u.is_empty()) {
        out.push(low(u));
    }
    out.extend(list_of(r, "emails").iter().map(|s| low(s)));
    out.extend(list_of(r, "aliases").iter().map(|s| low(s)));
    out.retain(|s| !s.is_empty());
    out.dedup();
    out
}

/// Whether `who` names this reviewer.
pub fn names(r: &Row, who: &str) -> bool {
    let w = low(who);
    !w.is_empty() && idents(r).contains(&w)
}

pub fn roster(app: &App, project: &str) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM reviewers WHERE project = ? ORDER BY lower(name), id", p![project])
}

/// The project's reviewer `who` names (by name, host id, email or alias).
pub fn find(app: &App, project: &str, who: &str) -> Result<Option<Row>> {
    Ok(roster(app, project)?.into_iter().find(|r| names(r, who)))
}

fn need(app: &App, project: &str, who: &str) -> Result<Row> {
    match find(app, project, who)? {
        Some(r) => Ok(r),
        None => err(404, format!("{project} has no reviewer “{who}”. tb reviewers list shows them; tb reviewers add adds one.")),
    }
}

pub fn get(app: &App, id: i64) -> Result<Row> {
    match app.db.q1("SELECT * FROM reviewers WHERE id = ?", p![id])? {
        Some(r) => Ok(r),
        None => err(404, "There's no such reviewer."),
    }
}

/// The reviewer the host knows as `user` on this project, if the roster has them.
pub fn by_host_user(app: &App, project: &str, user: &str) -> Result<Option<Row>> {
    if user.is_empty() {
        return Ok(None);
    }
    find(app, project, user)
}

/// The project a request is about: `project`, else the one `cwd` is in.
pub fn project_of(app: &App, body: &Value, query: Option<&crate::api::Query>) -> Result<String> {
    let mut p = body_str(body, "project");
    if p.is_empty() {
        p = query.and_then(|q| q.get("project")).cloned().unwrap_or_default();
    }
    if p.is_empty() {
        let cwd = body.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string()).or_else(|| query.and_then(|q| q.get("cwd")).cloned());
        p = crate::projects::project_for_path(app, cwd.as_deref())?.unwrap_or_default();
    }
    if p.is_empty() {
        return err(400, "Which project? Add --project <name>, or run it inside the project's folder.");
    }
    Ok(p)
}

fn push_unique(list: &mut Vec<String>, v: &str, not: &[String]) {
    let v = v.trim();
    if v.is_empty() || list.iter().any(|x| low(x) == low(v)) || not.iter().any(|x| low(x) == low(v)) {
        return;
    }
    list.push(v.to_string());
}

/// Who someone is, from any source: a name and whatever ids came with it.
#[derive(Debug, Clone, Default)]
pub struct Person {
    pub name: String,
    pub host_user: Option<String>,
    pub emails: Vec<String>,
    pub aliases: Vec<String>,
    pub slack: Option<String>,
    pub source: String,
    pub commits: Option<i64>,
}

/// Clearly a host account, not a nickname: `@ana-gh` or a Bitbucket `{uuid}`. A bare word (`ana-gh`,
/// `Nick`) could be either, so it's only a name.
pub fn clearly_host_id(s: &str) -> bool {
    let s = s.trim();
    let login = |l: &str| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (s.starts_with('{') && s.ends_with('}') && s.len() > 2) || s.strip_prefix('@').is_some_and(login)
}

/// Whether a reviewer who shares only a name with `p` is them: the owner adding them by name
/// (`tb reviewers add`), or one side is just a name. Two people with the same name but their own
/// host ids or emails stay apart.
fn same_by_name(r: &Row, p: &Person) -> bool {
    let (rh, ph) = (r.s("host_user").filter(|u| !u.is_empty()), p.host_user.as_deref().filter(|u| !u.is_empty()));
    if let (Some(a), Some(b)) = (rh, ph) {
        if low(a) != low(b) {
            return false;
        }
    }
    let bare_row = rh.is_none() && list_of(r, "emails").is_empty();
    let bare_p = ph.is_none() && p.emails.is_empty();
    p.source == "tb" || p.source.is_empty() || bare_row || bare_p
}

/// Adds a person to the project's roster, or folds what's new about them into the reviewer they
/// already are (a shared host id, email, alias or Slack id; a shared name only as `same_by_name`
/// allows). When they match several rows, those fold into an active one before a removed one. The
/// reviewer's row.
pub fn fold(app: &App, project: &str, p: &Person) -> Result<Row> {
    let mut keys: Vec<&str> = vec![];
    if let Some(u) = &p.host_user {
        keys.push(u);
    }
    keys.extend(p.emails.iter().map(|s| s.as_str()));
    keys.extend(p.aliases.iter().map(|s| s.as_str()));
    // A shared Slack id is the same person too, as the Python board folded them.
    let slack = p.slack.as_deref().map(low).filter(|s| !s.is_empty());
    let same_slack = |r: &Row| slack.as_deref().is_some_and(|s| r.s("slack").map(low).as_deref() == Some(s));
    let mut hits: Vec<Row> = roster(app, project)?
        .into_iter()
        .filter(|r| keys.iter().any(|k| names(r, k)) || (names(r, &p.name) && same_by_name(r, p)) || same_slack(r))
        .collect();
    // The row that stays: an active one over a removed one, then the most commits, then the oldest.
    hits.sort_by_key(|r| (r.s("removed_at").is_some_and(|s| !s.is_empty()), -r.i0("commits"), r.id()));
    let now = now_iso();
    let Some(first) = hits.first().cloned() else {
        let id = app.db.insert(
            "reviewers",
            fields!["project" => project, "name" => p.name.trim(), "host_user" => p.host_user.clone().filter(|u| !u.is_empty()),
                    "emails" => jdumps(&json!(p.emails)), "aliases" => jdumps(&json!(p.aliases)), "slack" => p.slack.clone(),
                    "source" => if p.source.is_empty() { "tb" } else { p.source.as_str() }, "commits" => p.commits.unwrap_or(0),
                    "created_at" => now, "updated_at" => now],
        )?;
        return get(app, id);
    };
    // Someone who matches two rows is one person: fold the others in too.
    for other in hits.drain(1..) {
        merge_rows(app, &first, &other)?;
    }
    let r = get(app, first.id())?;
    let mut emails = list_of(&r, "emails");
    let mut aliases = list_of(&r, "aliases");
    let mut host_user = r.s("host_user").filter(|u| !u.is_empty()).map(|s| s.to_string());
    let base = vec![r.st("name")];
    for e in &p.emails {
        push_unique(&mut emails, e, &[]);
    }
    if low(&p.name) != low(&r.st("name")) {
        push_unique(&mut aliases, &p.name, &emails);
    }
    match (&host_user, &p.host_user) {
        (None, Some(u)) if !u.is_empty() => host_user = Some(u.clone()),
        (Some(h), Some(u)) if low(h) != low(u) => push_unique(&mut aliases, u, &base),
        _ => {}
    }
    for a in &p.aliases {
        push_unique(&mut aliases, a, &emails);
    }
    aliases.retain(|a| low(a) != low(&r.st("name")) && host_user.as_deref().map(|h| low(h) != low(a)).unwrap_or(true));
    let mut f = fields!["emails" => jdumps(&json!(emails)), "aliases" => jdumps(&json!(aliases)), "host_user" => host_user, "updated_at" => now];
    if let Some(s) = p.slack.clone().filter(|s| !s.is_empty()) {
        f.push(("slack", json!(s)));
    }
    if let Some(c) = p.commits {
        f.push(("commits", json!(c)));
    }
    app.db.update("reviewers", &json!(r.id()), f)?;
    get(app, r.id())
}

/// Folds `other` into `keep`: its names become aliases, its asks and bot runs move over, and it goes.
fn merge_rows(app: &App, keep: &Row, other: &Row) -> Result<()> {
    if keep.id() == other.id() {
        return Ok(());
    }
    let keep = get(app, keep.id())?;
    let mut emails = list_of(&keep, "emails");
    let mut aliases = list_of(&keep, "aliases");
    for e in list_of(other, "emails") {
        push_unique(&mut emails, &e, &[]);
    }
    let mut host_user = keep.s("host_user").filter(|u| !u.is_empty()).map(|s| s.to_string());
    match (&host_user, other.s("host_user").filter(|u| !u.is_empty())) {
        (None, Some(u)) => host_user = Some(u.to_string()),
        (Some(h), Some(u)) if low(h) != low(u) => push_unique(&mut aliases, u, &[]),
        _ => {}
    }
    for a in std::iter::once(other.st("name")).chain(list_of(other, "aliases")) {
        push_unique(&mut aliases, &a, &emails);
    }
    aliases.retain(|a| low(a) != low(&keep.st("name")));
    let mut f = fields!["emails" => jdumps(&json!(emails)), "aliases" => jdumps(&json!(aliases)), "host_user" => host_user,
                        "pinned" => (keep.b("pinned") || other.b("pinned")) as i64, "updated_at" => now_iso(),
                        "commits" => keep.i0("commits") + other.i0("commits"),
                        "carried_asks" => keep.i0("carried_asks") + other.i0("carried_asks"),
                        "carried_swaps" => keep.i0("carried_swaps") + other.i0("carried_swaps")];
    let later = |a: Option<&str>, b: Option<&str>| a.and_then(parse_iso).unwrap_or(0.0) < b.and_then(parse_iso).unwrap_or(0.0);
    if later(keep.s("carried_last_ask"), other.s("carried_last_ask")) {
        f.push(("carried_last_ask", other.v("carried_last_ask")));
    }
    if keep.f("bot_every_h").is_none() && other.f("bot_every_h").is_some() {
        f.push(("bot_every_h", other.v("bot_every_h")));
        f.push(("bot_mark", other.v("bot_mark")));
    }
    if !has(keep.s("slack")) && has(other.s("slack")) {
        f.push(("slack", other.v("slack")));
    }
    app.db.update("reviewers", &json!(keep.id()), f)?;
    app.db.x("UPDATE review_asks SET reviewer_id = ? WHERE reviewer_id = ?", p![keep.id(), other.id()])?;
    app.db.x("UPDATE OR IGNORE reviewer_bot_runs SET reviewer_id = ? WHERE reviewer_id = ?", p![keep.id(), other.id()])?;
    app.db.x("DELETE FROM reviewer_bot_runs WHERE reviewer_id = ?", p![other.id()])?;
    app.db.x("DELETE FROM reviewers WHERE id = ?", p![other.id()])?;
    Ok(())
}

pub fn display(r: &Row) -> String {
    r.st("name")
}

/// The median of a reviewer's recent review times, in work minutes: their last `speed_asks` asks
/// from the last `speed_days` that say how fast they are. An answered ask counts its time; one
/// swapped off for not reviewing counts as `slow_cap_mins`; one still open counts its time so far
/// (up to the cap) once that's slower than the rest would make them, and takes no place until then.
pub fn median_work_mins(app: &App, id: i64) -> Result<Option<f64>> {
    let cfg = &app.cfg.reviewers;
    let since = iso(now_ts() - cfg.speed_days * 86400.0);
    let rows = app.db.q(
        "SELECT * FROM review_asks WHERE reviewer_id = ? AND asked_at >= ? AND (work_mins IS NOT NULL OR state IN ('swapped', 'open')) ORDER BY id DESC",
        p![id, since],
    )?;
    let n = cfg.speed_asks.max(1);
    let settled = |r: &Row| match (r.f("work_mins"), r.st("state").as_str()) {
        (Some(m), _) => Some(m),
        (None, "swapped") => Some(cfg.slow_cap_mins),
        _ => None,
    };
    // An open ask only ever makes them slower: it counts once it's taken longer than the speed they'd get without it.
    let pace = crate::picker::speed(cfg, median(&rows.iter().filter_map(settled).take(n).collect::<Vec<_>>()));
    let counted: Vec<f64> = rows
        .iter()
        .filter_map(|r| {
            settled(r).or_else(|| {
                let at = r.s("asked_at").and_then(parse_iso).unwrap_or_else(now_ts);
                let m = crate::picker::work_minutes(app, at, now_ts()).min(cfg.slow_cap_mins);
                (crate::picker::speed(cfg, Some(m)) < pace).then_some(m)
            })
        })
        .take(n)
        .collect();
    Ok(median(&counted))
}

fn median(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut xs = xs.to_vec();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = xs.len();
    Some(if n % 2 == 1 { xs[n / 2] } else { (xs[n / 2 - 1] + xs[n / 2]) / 2.0 })
}

/// A reviewer as the API, `tb reviewers list` and the app show them.
pub fn dict(app: &App, r: &Row) -> Result<Value> {
    let open = app.db.count("SELECT COUNT(*) FROM review_asks WHERE reviewer_id = ? AND state = 'open'", p![r.id()])?;
    let asks = ask_count(app, r)?;
    let last = last_asked(app, r)?;
    let swaps = app.db.count("SELECT COUNT(*) FROM review_asks WHERE reviewer_id = ? AND state IN ('swapped', 'came_back')", p![r.id()])? + r.i0("carried_swaps");
    let last_run = app.db.val("SELECT MAX(at) FROM reviewer_bot_runs WHERE reviewer_id = ?", p![r.id()])?;
    let next_run = crate::botrun::next_run(app, r)?.map(iso);
    Ok(json!({
        "id": r.id(), "project": r.v("project"), "name": r.v("name"), "user": r.v("host_user"),
        "emails": list_of(r, "emails"), "aliases": list_of(r, "aliases"), "slack": r.v("slack"), "source": r.v("source"),
        "commits": r.i0("commits"),
        "removed": r.s("removed_at").is_some(), "removed_at": r.v("removed_at"), "removed_why": r.v("removed_why"),
        "pinned": r.b("pinned"), "automation": r.f("automation").unwrap_or(1.0),
        "bot": if r.f("bot_every_h").is_some() { json!({"every_h": r.v("bot_every_h"), "mark": r.v("bot_mark"), "last_run": last_run, "next_run": next_run}) } else { Value::Null },
        "median_work_mins": median_work_mins(app, r.id())?, "open_asks": open, "asks": asks, "swaps": swaps, "last_asked": last,
    }))
}

fn list(app: &App, query: &crate::api::Query) -> Result<Value> {
    let project = query.get("project").cloned().unwrap_or_default();
    let mut project = project;
    if project.is_empty() {
        if let Some(cwd) = query.get("cwd").filter(|c| !c.is_empty()) {
            project = crate::projects::project_for_path(app, Some(cwd))?.unwrap_or_default();
        }
    }
    let rows = if project.is_empty() || project == "all" {
        app.db.q("SELECT * FROM reviewers ORDER BY project, lower(name), id", p![])?
    } else {
        roster(app, &project)?
    };
    let mut by_project: Vec<(String, Vec<Value>)> = vec![];
    for r in &rows {
        let d = dict(app, r)?;
        let p = r.st("project");
        match by_project.iter_mut().find(|(n, _)| *n == p) {
            Some((_, v)) => v.push(d),
            None => by_project.push((p, vec![d])),
        }
    }
    Ok(json!({
        "project": if project.is_empty() || project == "all" { Value::Null } else { json!(project) },
        "projects": by_project.into_iter().map(|(n, v)| json!({"name": n, "reviewers": v})).collect::<Vec<_>>(),
    }))
}

fn strs(body: &Value, key: &str) -> Vec<String> {
    str_list(body.get(key)).into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn who_of(body: &Value) -> String {
    let w = body_str(body, "who");
    if w.is_empty() { "tb".into() } else { w }
}

fn log(app: &App, text: &str) {
    app.info(format!("reviewers: {text}"));
}

fn add(app: &App, body: &Value) -> Result<Value> {
    let project = project_of(app, body, None)?;
    let name = body_str(body, "reviewer");
    if name.is_empty() {
        return err(400, "Name the reviewer: tb reviewers add \"Ana Lima\" --user ana-gh --email ana@acme.com");
    }
    let p = Person {
        name: name.clone(),
        host_user: Some(body_str(body, "user")).filter(|u| !u.is_empty()),
        emails: strs(body, "emails"),
        aliases: strs(body, "aliases"),
        slack: Some(body_str(body, "slack")).filter(|s| !s.is_empty()),
        source: "tb".into(),
        commits: None,
    };
    let r = fold(app, &project, &p)?;
    log(app, &format!("{} added {} to {project}'s reviewers", who_of(body), r.st("name")));
    dict(app, &r)
}

fn set(app: &App, r: &Row, f: Vec<(&str, Value)>) -> Result<Value> {
    let mut f = f;
    f.push(("updated_at", json!(now_iso())));
    app.db.update("reviewers", &json!(r.id()), f)?;
    dict(app, &get(app, r.id())?)
}

fn level(body: &Value) -> Result<f64> {
    let raw = body_str(body, "level").to_lowercase();
    if let Some((_, v)) = LEVELS.iter().find(|(n, _)| *n == raw) {
        return Ok(*v);
    }
    match raw.parse::<f64>() {
        Ok(v) if (0.1..=10.0).contains(&v) => Ok(v),
        _ => err(400, "Give the automation level as off, low, normal, high or a number from 0.1 to 10 (1 is normal)."),
    }
}

fn act(app: &App, action: &str, body: &Value) -> Result<Value> {
    let project = project_of(app, body, None)?;
    let r = need(app, &project, &body_str(body, "reviewer"))?;
    let out = match action {
        "remove" => {
            let why = Some(one_line(&body_str(body, "reason"), 200)).filter(|s| !s.is_empty());
            set(app, &r, fields!["removed_at" => now_iso(), "removed_why" => why])?
        }
        "back" => set(app, &r, fields!["removed_at" => null, "removed_why" => null])?,
        "pin" => set(app, &r, fields!["pinned" => as_bool(body.get("on"), true) as i64])?,
        "auto" => set(app, &r, fields!["automation" => level(body)?])?,
        "bot" => {
            if as_bool(body.get("off"), false) {
                set(app, &r, fields!["bot_every_h" => null, "bot_mark" => null])?
            } else {
                let every = body["every_h"].as_f64().or_else(|| body_str(body, "every_h").parse().ok());
                let mark = body_str(body, "mark");
                match every {
                    Some(h) if h > 0.0 && h <= 168.0 && !mark.is_empty() => set(app, &r, fields!["bot_every_h" => h, "bot_mark" => mark])?,
                    _ => return err(400, "Give how often their bot runs and the text it marks its comments with: --every 4 --mark \"AI review\"."),
                }
            }
        }
        "alias" => {
            let mut aliases = list_of(&r, "aliases");
            let emails = list_of(&r, "emails");
            let add = strs(body, "aliases");
            let user = body_str(body, "user").trim().trim_start_matches('@').to_string();
            if add.is_empty() && user.is_empty() {
                return err(400, "Give at least one alias (another name, email or host account of theirs), or their host account with --user.");
            }
            for a in add.iter().chain(Some(&user).filter(|u| !u.is_empty())) {
                if let Some(o) = find(app, &project, a)?.filter(|o| o.id() != r.id()) {
                    return err(409, format!("{a} already names {}. Fold them into one with tb reviewers merge \"{}\" \"{}\".", o.st("name"), r.st("name"), o.st("name")));
                }
            }
            let mut host_user = r.s("host_user").filter(|u| !u.is_empty()).map(|s| s.to_string());
            // --user is their host account; an old one stays one of their names.
            if !user.is_empty() {
                if let Some(old) = host_user.as_deref().filter(|h| low(h) != low(&user)) {
                    push_unique(&mut aliases, old, &[]);
                }
                aliases.retain(|a| low(a) != low(&user));
                host_user = Some(user.clone());
            }
            for a in &add {
                // Without a host account yet, one that's clearly a host id (`@login`, `{uuid}`) becomes
                // theirs; a bare word could be a nickname, so it's only a name (--user makes it the account).
                if host_user.is_none() && clearly_host_id(a) {
                    host_user = Some(a.trim().trim_start_matches('@').to_string());
                } else if a.contains('@') && !a.starts_with('@') {
                    push_unique(&mut aliases, a, &emails);
                } else {
                    push_unique(&mut aliases, a, host_user.as_slice());
                }
            }
            set(app, &r, fields!["aliases" => jdumps(&json!(aliases)), "host_user" => host_user])?
        }
        "merge" => {
            let other = need(app, &project, &body_str(body, "other"))?;
            if other.id() == r.id() {
                return err(400, "That's the same reviewer.");
            }
            merge_rows(app, &r, &other)?;
            dict(app, &get(app, r.id())?)?
        }
        _ => return err(404, "There's nothing at that address."),
    };
    log(app, &format!("{} ran {action} on {} ({project})", who_of(body), r.st("name")));
    Ok(out)
}

/// `tb reviewers sync`: commit authors join the roster now, and host accounts are matched.
fn sync(app: &App, body: &Value) -> Result<Value> {
    let project = project_of(app, body, None)?;
    let repo = crate::projects::project_path(app, Some(&project))?;
    let last = app.db.q1("SELECT * FROM tasks WHERE project = ? AND pr_num IS NOT NULL AND pr_repo IS NOT NULL ORDER BY id DESC LIMIT 1", p![project])?;
    let pr = last.as_ref().and_then(crate::prhost::PrRef::of);
    let mut out = crate::picker::sync(app, &project, repo.as_deref(), pr.as_ref(), true)?;
    out["project"] = json!(project);
    out["reviewers"] = json!(roster(app, &project)?.iter().map(|r| dict(app, r)).collect::<Result<Vec<_>>>()?);
    Ok(out)
}

/// `/reviewers…`.
pub fn route(app: &App, method: &str, rest: &[&str], query: &crate::api::Query, body: &Value) -> Result<Value> {
    match (method, rest) {
        ("GET", []) => list(app, query),
        ("POST", []) => app.db.tx(|| add(app, body)),
        ("POST", ["sync"]) => sync(app, body),
        ("POST", [action]) => app.db.tx(|| act(app, action, body)),
        _ => err(404, "There's nothing at that address."),
    }
}

/// Records that `user` was asked to review a task's PR. The ask's id.
pub fn record_ask(app: &App, t: &Row, user: &str, name: &str, why: &str, by: &str, replaces: Option<i64>) -> Result<i64> {
    let project = t.st("project");
    let r = by_host_user(app, &project, user)?;
    let name = r.as_ref().map(display).unwrap_or_else(|| if name.is_empty() { user.to_string() } else { name.to_string() });
    app.db.insert(
        "review_asks",
        fields!["task_id" => t.id(), "project" => project, "pr_host" => t.v("pr_host"), "pr_repo" => t.v("pr_repo"), "pr_num" => t.v("pr_num"),
                "reviewer_id" => r.as_ref().map(|r| r.id()), "host_user" => user, "name" => name, "why" => why, "asked_by" => by,
                "replaces" => replaces, "state" => "open", "asked_at" => now_iso()],
    )
}

/// The open ask of `user` on this task, if there is one.
pub fn open_ask(app: &App, task_id: i64, user: &str) -> Result<Option<Row>> {
    app.db.q1("SELECT * FROM review_asks WHERE task_id = ? AND lower(host_user) = lower(?) AND state = 'open' ORDER BY id DESC LIMIT 1", p![task_id, user])
}

/// Every ask on this task, oldest first.
pub fn asks_of(app: &App, task_id: i64) -> Result<Vec<Row>> {
    app.db.q("SELECT * FROM review_asks WHERE task_id = ? ORDER BY id", p![task_id])
}

pub fn close_ask(app: &App, id: i64, state: &str) -> Result<()> {
    app.db.update("review_asks", &json!(id), fields!["state" => state, "closed_at" => now_iso()])
}

pub fn ask_dict(a: &Row) -> Value {
    json!({"id": a.id(), "user": a.v("host_user"), "name": a.v("name"), "why": a.v("why"), "by": a.v("asked_by"), "state": a.v("state"),
           "asked_at": a.v("asked_at"), "answered_at": a.v("answered_at"), "answer": a.v("answer"), "work_mins": a.v("work_mins"),
           "replaces": a.v("replaces")})
}

/// Notes a board event about the task's reviewers (outside any host call).
pub fn note(app: &App, t: &Row, who: &str, text: &str) -> Result<()> {
    board::log_event(app, t.id(), who, "status", text).map(|_| ())
}
