//! Whether a reviewer is around to review now, for the picker's tiers (`picker.rs`). Optional:
//! `[reviewers] availability = "slack"` turns it on.
//!
//! Tiers, best first: online (active on Slack, or posted today), quiet (on Slack but neither),
//! off (outside their own working hours, `local_start`–`local_end` in their Slack time zone, or a
//! weekend), and out (their Slack status matches `out_pattern`): never picked. Someone Slack doesn't
//! know is taken off the roster with `drop_not_on_slack` ("not on Slack"; `tb reviewers back` undoes
//! it), else skipped. The picker checks at most `pick_tries` candidates per pick, in turn order,
//! and takes the first online one, else the best tier it saw. Outside the board's work hours it
//! checks nobody.
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
pub const READ_ONLY: &[&str] = &["users.lookupByEmail", "users.info", "users.getPresence", "search.messages"];

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
}

impl SlackAvailability {
    fn user_id(&self, r: &Row) -> std::result::Result<Option<String>, String> {
        let slack = r.st("slack");
        let looks_like_id = |s: &str| s.len() > 6 && (s.starts_with('U') || s.starts_with('W')) && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if looks_like_id(&slack) {
            return Ok(Some(slack));
        }
        let mut emails: Vec<String> = if slack.contains('@') { vec![slack] } else { vec![] };
        emails.extend(jloads_arr(r.s("emails")).iter().filter_map(|e| e.as_str().map(|s| s.to_string())).filter(|e| !e.ends_with("@users.noreply.github.com")));
        for e in emails {
            match self.api.get("users.lookupByEmail", &[("email", e)]) {
                Ok(v) => return Ok(v["user"]["id"].as_str().map(|s| s.to_string())),
                Err(e) if e == "users_not_found" => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }
}

impl Availability for SlackAvailability {
    fn check(&self, r: &Row) -> std::result::Result<Presence, String> {
        let Some(id) = self.user_id(r)? else { return Ok(Presence { tier: Tier::Missing, why: "not on Slack".into() }) };
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
            Some(Arc::new(SlackAvailability { api: Box::new(SlackHttp::new(&token)), cfg: app.cfg.reviewers.clone() }))
        }
        _ => None,
    }
}

/// A reviewer's presence now (cached for `cache_mins`); Unknown outside work hours or without a provider.
pub fn of(app: &App, r: &Row) -> Presence {
    let unknown = |why: &str| Presence { tier: Tier::Unknown, why: why.into() };
    let Some(p) = provider(app) else { return unknown("no availability check") };
    if !hours::is_open(app) {
        return unknown("outside work hours");
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
    if provider(app).is_none() || !hours::is_open(app) {
        return Ok(pool.first().copied());
    }
    let mut seen: Option<(u8, usize)> = None;
    let mut checked = 0;
    for &i in pool {
        if checked >= app.cfg.reviewers.pick_tries.max(1) {
            // Past the tries: someone unchecked beats the off and the quiet.
            return Ok(match seen {
                Some((r, j)) if r <= Tier::Unknown.rank().unwrap_or(1) => Some(j),
                _ => Some(i),
            });
        }
        checked += 1;
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
        SlackAvailability { api: Box::new(Canned(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())), cfg: Default::default() }
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
        let a = slack(vec![("users.lookupByEmail ana@acme.com", json!({"ok": false, "error": "users_not_found"}))]);
        assert_eq!(a.check(&reviewer("ana@acme.com")).unwrap().tier, Tier::Missing);
    }

    #[test]
    fn the_slack_client_only_reads() {
        let s = SlackHttp::new("xoxb-1");
        assert!(s.get("chat.postMessage", &[]).unwrap_err().contains("doesn't call"));
    }
}
