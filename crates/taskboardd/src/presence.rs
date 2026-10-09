//! Whether a reviewer is around to review now, for the picker's tiers (`picker.rs`). Optional:
//! `[reviewers] availability = "slack"` turns it on.
//!
//! Tiers, best first: online (active on Slack, or posted today, or away before `quiet_from` where
//! they are: starting their day), quiet (on Slack but none of those), off (outside their own working
//! hours, `local_start`–`local_end` in their Slack time zone, or a weekend), and out (their Slack
//! status matches `out_pattern`): never picked. People are found on Slack by their Slack id or
//! email, else by name, else by their host login or the part of their email (or noreply login)
//! before the @. Someone none of those finds is taken off the roster with `drop_not_on_slack` ("not
//! on Slack"; `tb reviewers back` undoes it), else skipped; someone a lookup couldn't be tried for
//! (Slack refused the user list), or who fits more than one person on Slack, is neither. The picker checks at most `pick_tries` candidates per pick, in turn
//! order, and takes the first online one, else the best tier it saw. Outside the board's work hours
//! it checks nobody, but an out status seen within `out_keeps_hours` still holds.
//!
//! The board never messages anyone: the Slack client only calls the read methods in [`READ_ONLY`]
//! (with Taskboard's Slack account, Settings ▸ Accounts), and refuses every other.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Timelike};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::app::App;
use crate::picker::Pick;
use crate::util::*;
use crate::{fields, hours, reviewers};

/// The Slack methods the board may call.
pub const READ_ONLY: &[&str] = &["users.lookupByEmail", "users.list", "users.info", "users.getPresence", "search.messages"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Online,
    Quiet,
    Off,
    Out,
    /// The provider doesn't know them.
    Missing,
    /// Not checked (no provider, outside work hours, or past `pick_tries`).
    Unknown,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Online => "online",
            Tier::Quiet => "quiet",
            Tier::Off => "off",
            Tier::Out => "out",
            Tier::Missing => "missing",
            Tier::Unknown => "unknown",
        }
    }
    /// Lower is better; None: never picked.
    fn rank(self) -> Option<u8> {
        match self {
            Tier::Online => Some(0),
            Tier::Unknown => Some(1),
            Tier::Quiet => Some(2),
            Tier::Off => Some(3),
            Tier::Out | Tier::Missing => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Presence {
    pub tier: Tier,
    /// Why, for the log ("status: On vacation", "6:40 PM where they are").
    pub why: String,
}

/// Something that knows whether people are around: Slack, or a fake in tests.
pub trait Availability: Send + Sync {
    fn check(&self, r: &Row) -> std::result::Result<Presence, String>;
}

/// One Slack Web API call (a GET with the token); the reply when `ok`, else Slack's error code.
pub trait SlackApi: Send + Sync {
    fn get(&self, method: &str, params: &[(&str, String)]) -> std::result::Result<Value, String>;
}

pub struct SlackHttp {
    token: String,
    agent: ureq::Agent,
}

impl SlackHttp {
    pub fn new(token: &str) -> SlackHttp {
        SlackHttp { token: token.to_string(), agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build() }
    }
}

impl SlackApi for SlackHttp {
    fn get(&self, method: &str, params: &[(&str, String)]) -> std::result::Result<Value, String> {
        if !READ_ONLY.contains(&method) {
            return Err(format!("the board doesn't call Slack's {method}"));
        }
        let mut req = self.agent.get(&format!("https://slack.com/api/{method}")).set("Authorization", &format!("Bearer {}", self.token));
        for (k, v) in params {
            req = req.query(k, v);
        }
        let v: Value = req.call().map_err(|_| "Can't reach Slack".to_string())?.into_json().map_err(|_| "Slack answered something that isn't JSON".to_string())?;
        if v["ok"] == true {
            Ok(v)
        } else {
            Err(v["error"].as_str().unwrap_or("Slack refused").to_string())
        }
    }
}

/// `HH:MM` as minutes after midnight.
fn hm(s: &str) -> i64 {
    let (h, m) = s.split_once(':').unwrap_or((s, "0"));
    h.trim().parse::<i64>().unwrap_or(0) * 60 + m.trim().parse::<i64>().unwrap_or(0)
}

pub struct SlackAvailability {
    pub api: Box<dyn SlackApi>,
    pub cfg: reviewers::ReviewersConfig,
    /// The workspace's people (`users.list`) and when they were read, for the name lookups.
    pub people: Mutex<Option<(f64, Vec<Value>)>>,
}

/// What looking someone up on Slack found.
enum Lookup {
    Found(String),
    /// Every lookup ran and none found them.
    NotFound,
    /// A lookup couldn't run (Slack refused the user list) or fit more than one person, so nobody
    /// can say they're not there. Why, for the log.
    Untried(&'static str),
}

/// What one way of matching someone in the workspace's people found.
enum Hits {
    None,
    One(String),
    Many,
}

fn hits(found: Vec<&Value>) -> Hits {
    match found.as_slice() {
        [] => Hits::None,
        [u] => u["id"].as_str().map(|s| Hits::One(s.to_string())).unwrap_or(Hits::None),
        _ => Hits::Many,
    }
}

const NOREPLY: &str = "@users.noreply.github.com";

impl SlackAvailability {
    pub fn new(api: Box<dyn SlackApi>, cfg: reviewers::ReviewersConfig) -> SlackAvailability {
        SlackAvailability { api, cfg, people: Mutex::new(None) }
    }

    /// Everyone in the workspace (not deleted, not bots), read at most every `cache_mins`.
    fn people(&self) -> std::result::Result<Vec<Value>, String> {
        if let Some((at, list)) = self.people.lock().as_ref() {
            if now_ts() - at < self.cfg.cache_mins * 60.0 {
                return Ok(list.clone());
            }
        }
        let mut out = vec![];
        let mut cursor = String::new();
        for _ in 0..50 {
            let mut params = vec![("limit", "200".to_string())];
            if !cursor.is_empty() {
                params.push(("cursor", cursor.clone()));
            }
            let v = self.api.get("users.list", &params)?;
            out.extend(v["members"].as_array().cloned().unwrap_or_default().into_iter().filter(|u| u["deleted"] != true && u["is_bot"] != true));
            cursor = v["response_metadata"]["next_cursor"].as_str().unwrap_or("").to_string();
            if cursor.is_empty() {
                break;
            }
        }
        *self.people.lock() = Some((now_ts(), out.clone()));
        Ok(out)
    }

    fn user_id(&self, r: &Row) -> std::result::Result<Lookup, String> {
        let slack = r.st("slack");
        let looks_like_id = |s: &str| s.len() > 6 && (s.starts_with('U') || s.starts_with('W')) && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if looks_like_id(&slack) {
            return Ok(Lookup::Found(slack));
        }
        let all: Vec<String> = jloads_arr(r.s("emails")).iter().filter_map(|e| e.as_str().map(|s| s.to_string())).collect();
        let mut emails: Vec<String> = if slack.contains('@') { vec![slack] } else { vec![] };
        emails.extend(all.iter().filter(|e| !e.ends_with(NOREPLY)).cloned());
        for e in emails {
            match self.api.get("users.lookupByEmail", &[("email", e)]) {
                Ok(v) => {
                    if let Some(id) = v["user"]["id"].as_str() {
                        return Ok(Lookup::Found(id.to_string()));
                    }
                }
                Err(e) if e == "users_not_found" => continue,
                Err(e) => return Err(e),
            }
        }
        let people = match self.people() {
            Ok(p) => p,
            Err(_) => return Ok(Lookup::Untried("couldn't look them up on Slack")),
        };
        // A match that fits more than one person finds nobody, but doesn't say they're not there.
        let mut ambiguous = false;
        // By full name (theirs, or another name of theirs that isn't an email).
        let mut names = vec![r.st("name").trim().to_lowercase()];
        names.extend(jloads_arr(r.s("aliases")).iter().filter_map(|a| a.as_str()).filter(|a| !a.contains('@') && a.contains(' ')).map(|a| a.trim().to_lowercase()));
        names.retain(|n| !n.is_empty());
        let named = |u: &Value| {
            [&u["real_name"], &u["profile"]["real_name"], &u["profile"]["display_name"]].iter().filter_map(|v| v.as_str()).any(|n| names.contains(&n.trim().to_lowercase()))
        };
        match hits(people.iter().filter(|u| named(u)).collect()) {
            Hits::One(id) => return Ok(Lookup::Found(id)),
            Hits::Many => ambiguous = true,
            Hits::None => {}
        }
        // By the part of their email before the @ (a noreply email's login), or their host login.
        let mut prefixes: Vec<String> = all
            .iter()
            .filter_map(|e| {
                let local = e.split('@').next().unwrap_or("");
                let local = if e.ends_with(NOREPLY) { local.split_once('+').map(|(_, l)| l).unwrap_or(local) } else { local };
                Some(local.trim().to_lowercase()).filter(|l| !l.is_empty())
            })
            .collect();
        // A Bitbucket `{uuid}` names nobody on Slack.
        if let Some(u) = r.s("host_user").map(|u| u.trim().trim_start_matches('@').to_lowercase()).filter(|u| !u.is_empty() && !u.starts_with('{')) {
            if !prefixes.contains(&u) {
                prefixes.push(u);
            }
        }
        let prefixed = |u: &Value| {
            let email = u["profile"]["email"].as_str().unwrap_or("").split('@').next().unwrap_or("").to_lowercase();
            let handle = u["name"].as_str().unwrap_or("").to_lowercase();
            prefixes.iter().any(|p| *p == email || *p == handle)
        };
        match hits(people.iter().filter(|u| prefixed(u)).collect()) {
            Hits::One(id) => return Ok(Lookup::Found(id)),
            Hits::Many => ambiguous = true,
            Hits::None => {}
        }
        Ok(if ambiguous { Lookup::Untried("more than one person on Slack fits them") } else { Lookup::NotFound })
    }
}

impl Availability for SlackAvailability {
    fn check(&self, r: &Row) -> std::result::Result<Presence, String> {
        let id = match self.user_id(r)? {
            Lookup::Found(id) => id,
            Lookup::NotFound => return Ok(Presence { tier: Tier::Missing, why: "not on Slack".into() }),
            Lookup::Untried(why) => return Ok(Presence { tier: Tier::Unknown, why: why.into() }),
        };
        let info = self.api.get("users.info", &[("user", id.clone())])?;
        let u = &info["user"];
        if u["deleted"] == true {
            return Ok(Presence { tier: Tier::Missing, why: "not on Slack".into() });
        }
        let status = format!("{} {}", u["profile"]["status_emoji"].as_str().unwrap_or(""), u["profile"]["status_text"].as_str().unwrap_or("")).trim().to_string();
        if !status.is_empty() {
            if let Ok(re) = regex::Regex::new(&self.cfg.out_pattern) {
                if re.is_match(&status) {
                    return Ok(Presence { tier: Tier::Out, why: format!("status: {status}") });
                }
            }
        }
        let offset = u["tz_offset"].as_i64().unwrap_or(0);
        let utc = chrono::DateTime::from_timestamp(now_ts() as i64, 0).unwrap_or_else(chrono::Utc::now).naive_utc();
        let local = utc + chrono::Duration::seconds(offset);
        let mins = (local.hour() * 60 + local.minute()) as i64;
        let weekend = matches!(local.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        if (self.cfg.weekends_off && weekend) || mins < hm(&self.cfg.local_start) || mins >= hm(&self.cfg.local_end) {
            let clock = twelve_hour(&local.format("%H:%M").to_string());
            return Ok(Presence { tier: Tier::Off, why: format!("{clock} where they are") });
        }
        let presence = self.api.get("users.getPresence", &[("user", id.clone())])?;
        if presence["presence"] == "active" {
            return Ok(Presence { tier: Tier::Online, why: "active on Slack".into() });
        }
        // Posted today (needs search:read; without it, this is skipped).
        if let Ok(v) = self.api.get("search.messages", &[("query", format!("from:<@{id}> on:{}", local.format("%Y-%m-%d"))), ("count", "1".into())]) {
            if v["messages"]["total"].as_i64().unwrap_or(0) > 0 {
                return Ok(Presence { tier: Tier::Online, why: "posted on Slack today".into() });
            }
        }
        if mins < hm(&self.cfg.quiet_from) {
            return Ok(Presence { tier: Tier::Online, why: "starting their day".into() });
        }
        Ok(Presence { tier: Tier::Quiet, why: "away on Slack".into() })
    }
}

static INSTALLED: Lazy<Mutex<HashMap<PathBuf, Arc<dyn Availability>>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static CACHE: Lazy<Mutex<HashMap<String, (f64, Presence)>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Uses `a` for this board's availability checks (tests).
pub fn install(app: &App, a: Arc<dyn Availability>) {
    INSTALLED.lock().insert(app.cfg.data.clone(), a);
}

/// This board's availability provider, if it has one turned on.
pub fn provider(app: &App) -> Option<Arc<dyn Availability>> {
    if app.cfg.reviewers.availability.trim().is_empty() {
        return None;
    }
    if let Some(a) = INSTALLED.lock().get(&app.cfg.data) {
        return Some(a.clone());
    }
    match app.cfg.reviewers.availability.trim() {
        "slack" => {
            let (_, token) = crate::accounts::board_credentials(app, crate::accounts::Provider::Slack)?;
            Some(Arc::new(SlackAvailability::new(Box::new(SlackHttp::new(&token)), app.cfg.reviewers.clone())))
        }
        _ => None,
    }
}

fn out_key(r: &Row) -> String {
    format!("reviewer_out:{}", r.id())
}

/// The out status last seen for them, if it's within `out_keeps_hours`.
fn kept_out(app: &App, r: &Row) -> Option<Presence> {
    let v = jloads_obj(app.db.get_setting(&out_key(r)).ok().flatten().as_deref());
    let at = v.get("at").and_then(|a| a.as_str()).and_then(parse_iso)?;
    (now_ts() - at < app.cfg.reviewers.out_keeps_hours * 3600.0).then(|| Presence { tier: Tier::Out, why: v.get("why").and_then(|w| w.as_str()).unwrap_or("out").to_string() })
}

/// A reviewer's presence now (cached for `cache_mins`); Unknown outside work hours (unless they were
/// out recently) or without a provider.
pub fn of(app: &App, r: &Row) -> Presence {
    let unknown = |why: &str| Presence { tier: Tier::Unknown, why: why.into() };
    let Some(p) = provider(app) else { return unknown("no availability check") };
    if !hours::is_open(app) {
        return kept_out(app, r).unwrap_or_else(|| unknown("outside work hours"));
    }
    let key = format!("{}:{}", app.cfg.data.display(), r.id());
    if let Some((at, pr)) = CACHE.lock().get(&key).cloned() {
        if now_ts() - at < app.cfg.reviewers.cache_mins * 60.0 {
            return pr;
        }
    }
    let pr = match p.check(r) {
        Ok(pr) => pr,
        Err(e) => {
            app.info(format!("reviewers: couldn't check {} on Slack: {e}", r.st("name")));
            unknown("couldn't check")
        }
    };
    CACHE.lock().insert(key, (now_ts(), pr.clone()));
    // Remember an out status for after hours; anything else they answered clears it.
    let seen = match pr.tier {
        Tier::Out => Some(jdumps(&json!({"at": now_iso(), "why": pr.why}))),
        Tier::Unknown => return pr,
        _ => None,
    };
    if let Err(e) = app.db.set_setting(&out_key(r), seen.as_deref()) {
        app.info(format!("reviewers: couldn't note {}'s Slack status: {e}", r.st("name")));
    }
    pr
}

/// Takes a reviewer the provider doesn't know off the roster (with `drop_not_on_slack`).
fn drop_missing(app: &App, r: &Row, why: &str) -> Result<()> {
    if !app.cfg.reviewers.drop_not_on_slack || r.s("removed_at").is_some() {
        return Ok(());
    }
    app.info(format!("reviewers: {} is {why}; took them off {}'s roster", r.st("name"), r.st("project")));
    app.db.tx(|| app.db.update("reviewers", &json!(r.id()), fields!["removed_at" => now_iso(), "removed_why" => why, "updated_at" => now_iso()]))
}

/// The candidate to take from `pool` (indexes into `cands`, in turn order): the first one online
/// among the first `pick_tries` checked, else the best tier seen; never one who's out. Sets each
/// checked candidate's `tier`.
pub fn best(app: &App, cands: &mut [Pick], pool: &[usize]) -> Result<Option<usize>> {
    if pool.is_empty() {
        return Ok(None);
    }
    if provider(app).is_none() {
        return Ok(pool.first().copied());
    }
    if !hours::is_open(app) {
        // Nobody's checked; only someone recently out is passed over.
        for &i in pool {
            let pr = of(app, &cands[i].reviewer);
            cands[i].tier = pr.tier.name().to_string();
            if pr.tier != Tier::Out {
                return Ok(Some(i));
            }
        }
        return Ok(None);
    }
    let mut seen: Option<(u8, usize)> = None;
    for (checked, &i) in pool.iter().enumerate() {
        if checked >= app.cfg.reviewers.pick_tries.max(1) {
            // Past the tries: someone unchecked beats the off and the quiet.
            return Ok(match seen {
                Some((r, j)) if r <= Tier::Unknown.rank().unwrap_or(1) => Some(j),
                _ => Some(i),
            });
        }
        let pr = of(app, &cands[i].reviewer);
        cands[i].tier = pr.tier.name().to_string();
        if pr.tier == Tier::Missing {
            drop_missing(app, &cands[i].reviewer, &pr.why)?;
        }
        let Some(rank) = pr.tier.rank() else { continue };
        if rank == 0 {
            return Ok(Some(i));
        }
        if seen.map(|(r, _)| rank < r).unwrap_or(true) {
            seen = Some((rank, i));
        }
    }
    Ok(seen.map(|(_, i)| i))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Canned(HashMap<String, Value>);
    impl SlackApi for Canned {
        fn get(&self, method: &str, params: &[(&str, String)]) -> std::result::Result<Value, String> {
            let key = format!("{method} {}", params.first().map(|p| p.1.as_str()).unwrap_or(""));
            match self.0.get(&key) {
                Some(v) if v["ok"] == false => Err(v["error"].as_str().unwrap_or("").to_string()),
                Some(v) => Ok(v.clone()),
                None => Err("not_canned".into()),
            }
        }
    }

    fn reviewer(email: &str) -> Row {
        let mut r = Row::new();
        r.insert("id".into(), json!(1));
        r.insert("name".into(), json!("Ana"));
        r.insert("emails".into(), json!(format!("[\"{email}\"]")));
        r
    }

    fn slack(pairs: Vec<(&str, Value)>) -> SlackAvailability {
        SlackAvailability::new(Box::new(Canned(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())), Default::default())
    }

    /// Their Slack time zone offset that puts them at `hour` local time now, on a weekday.
    fn offset_for(hour: u32) -> i64 {
        let now = chrono::Utc::now().naive_utc();
        let mut off = (hour as i64 - now.hour() as i64) * 3600 - now.minute() as i64 * 60 + 60;
        let local = now + chrono::Duration::seconds(off);
        if matches!(local.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun) {
            off += if local.weekday() == chrono::Weekday::Sat { 2 } else { 1 } * 86400;
        }
        off
    }

    fn user(status: &str, hour: u32) -> Value {
        json!({"ok": true, "user": {"id": "U1", "tz_offset": offset_for(hour), "profile": {"status_text": status, "status_emoji": ""}}})
    }

    #[test]
    fn reads_the_tiers_from_slack() {
        let found = json!({"ok": true, "user": {"id": "U1"}});
        let a = slack(vec![("users.lookupByEmail ana@acme.com", found.clone()), ("users.info U1", user("", 11)), ("users.getPresence U1", json!({"ok": true, "presence": "active"}))]);
        assert_eq!(a.check(&reviewer("ana@acme.com")).unwrap().tier, Tier::Online);
        let a = slack(vec![("users.lookupByEmail ana@acme.com", found.clone()), ("users.info U1", user("", 11)), ("users.getPresence U1", json!({"ok": true, "presence": "away"}))]);
        assert_eq!(a.check(&reviewer("ana@acme.com")).unwrap().tier, Tier::Quiet, "away, and search isn't allowed");
        let a = slack(vec![("users.lookupByEmail ana@acme.com", found.clone()), ("users.info U1", user("On vacation", 11))]);
        assert_eq!(a.check(&reviewer("ana@acme.com")).unwrap(), Presence { tier: Tier::Out, why: "status: On vacation".into() });
        let a = slack(vec![("users.lookupByEmail ana@acme.com", found), ("users.info U1", user("", 21))]);
        assert_eq!(a.check(&reviewer("ana@acme.com")).unwrap().tier, Tier::Off, "9 PM where they are");
        let a = slack(vec![("users.lookupByEmail ana@acme.com", json!({"ok": false, "error": "users_not_found"})), ("users.list 200", json!({"ok": true, "members": []}))]);
        assert_eq!(a.check(&reviewer("ana@acme.com")).unwrap().tier, Tier::Missing);
    }

    #[test]
    fn away_early_in_their_day_isnt_quiet() {
        let found = json!({"ok": true, "user": {"id": "U1"}});
        let a = slack(vec![("users.lookupByEmail ana@acme.com", found), ("users.info U1", user("", 9)), ("users.getPresence U1", json!({"ok": true, "presence": "away"}))]);
        assert_eq!(a.check(&reviewer("ana@acme.com")).unwrap(), Presence { tier: Tier::Online, why: "starting their day".into() });
    }

    #[test]
    fn people_without_a_work_email_are_found_by_name_or_email_prefix() {
        let people = json!({"ok": true, "members": [
            {"id": "U1", "real_name": "Ana Lima", "name": "alima", "profile": {"email": "alima@acme.com"}},
            {"id": "U2", "real_name": "Bo Park", "name": "bo", "profile": {"email": "bo.park@acme.com"}},
            {"id": "U3", "real_name": "Old Bot", "is_bot": true, "name": "ana-gh", "profile": {}},
        ]});
        let info = |id: &str| json!({"ok": true, "user": {"id": id, "tz_offset": offset_for(11), "profile": {"status_text": "", "status_emoji": ""}}});
        let a = slack(vec![("users.list 200", people.clone()), ("users.info U1", info("U1")), ("users.getPresence U1", json!({"ok": true, "presence": "active"}))]);
        // Only a noreply email: found by name.
        let mut ana = reviewer("9+ana-gh@users.noreply.github.com");
        ana.insert("name".into(), json!("ana lima"));
        assert_eq!(a.check(&ana).unwrap().tier, Tier::Online);
        // Another name, found by the part of the email before the @ (the Slack handle).
        let a = slack(vec![("users.list 200", people), ("users.info U2", info("U2")), ("users.getPresence U2", json!({"ok": true, "presence": "active"}))]);
        let mut bo = reviewer("9+bo@users.noreply.github.com");
        bo.insert("name".into(), json!("Robert"));
        assert_eq!(a.check(&bo).unwrap().tier, Tier::Online);
        // Slack refuses the user list: they can't be said to be missing.
        let a = slack(vec![("users.list 200", json!({"ok": false, "error": "missing_scope"}))]);
        assert_eq!(a.check(&bo).unwrap().tier, Tier::Unknown);
    }

    #[test]
    fn a_host_login_finds_them_and_two_matches_are_not_missing() {
        let people = json!({"ok": true, "members": [
            {"id": "U1", "real_name": "Ana Lima", "name": "alima", "profile": {"email": "alima@acme.com"}},
            {"id": "U2", "real_name": "Ana Lima", "name": "ana2", "profile": {"email": "ana2@acme.com"}},
            {"id": "U3", "real_name": "Cy Ng", "name": "cyng", "profile": {"email": "cy.ng@acme.com"}},
        ]});
        let info = json!({"ok": true, "user": {"id": "U3", "tz_offset": offset_for(11), "profile": {"status_text": "", "status_emoji": ""}}});
        let a = slack(vec![("users.list 200", people.clone()), ("users.info U3", info), ("users.getPresence U3", json!({"ok": true, "presence": "active"}))]);
        // Host-only, under a name Slack doesn't have: found by their login as the Slack handle.
        let mut cy = Row::new();
        cy.insert("id".into(), json!(3));
        cy.insert("name".into(), json!("Cyrus"));
        cy.insert("host_user".into(), json!("cyng"));
        assert_eq!(a.check(&cy).unwrap().tier, Tier::Online);
        // A name that fits two people is unknown, not "not on Slack".
        let a = slack(vec![("users.list 200", people)]);
        let mut ana = Row::new();
        ana.insert("id".into(), json!(1));
        ana.insert("name".into(), json!("Ana Lima"));
        assert_eq!(a.check(&ana).unwrap(), Presence { tier: Tier::Unknown, why: "more than one person on Slack fits them".into() });
    }

    #[test]
    fn the_slack_client_only_reads() {
        let s = SlackHttp::new("xoxb-1");
        assert!(s.get("chat.postMessage", &[]).unwrap_err().contains("doesn't call"));
    }
}
