use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, Local, TimeZone, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value};

pub type Row = Map<String, Value>;
pub type Result<T> = std::result::Result<T, ApiError>;

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        ApiError { status, message: message.into() }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        ApiError::new(500, format!("The board's database failed: {e}"))
    }
}

pub fn err<T>(status: u16, message: impl Into<String>) -> Result<T> {
    Err(ApiError::new(status, message))
}

pub const KIND_LABELS: &[(&str, &str)] =
    &[("bug", "Bug"), ("gap", "Test gap"), ("follow", "Follow-up"), ("clean", "Clean-up")];
pub const ISSUE_KINDS: &[&str] = &["bug", "gap", "follow", "clean"];
pub const NOTE_KINDS: &[&str] = &["finding", "decision", "reference"];

pub fn kind_label(kind: &str) -> String {
    KIND_LABELS.iter().find(|(k, _)| *k == kind).map(|(_, l)| l.to_string()).unwrap_or_else(|| kind.to_string())
}

static CLOCK_OFFSET_MS: AtomicI64 = AtomicI64::new(0);

pub fn now_ts() -> f64 {
    let real = Utc::now().timestamp_millis() as f64 / 1000.0;
    real + CLOCK_OFFSET_MS.load(Ordering::Relaxed) as f64 / 1000.0
}

pub fn advance_clock(seconds: f64) {
    CLOCK_OFFSET_MS.fetch_add((seconds * 1000.0) as i64, Ordering::Relaxed);
}

pub fn reset_clock() {
    CLOCK_OFFSET_MS.store(0, Ordering::Relaxed);
}

pub fn iso(ts: f64) -> String {
    let dt = Utc.timestamp_opt(ts.floor() as i64, 0).single().unwrap_or_else(Utc::now);
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn now_iso() -> String {
    iso(now_ts())
}

pub fn parse_iso(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(&s.replace(' ', "T")) {
        return Some(d.timestamp_millis() as f64 / 1000.0);
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(n.and_utc().timestamp() as f64);
        }
    }
    None
}

/// Seconds since `s` that the Mac was awake: what waits and timeouts measure.
pub fn age_secs(s: Option<&str>) -> Option<f64> {
    s.and_then(parse_iso).map(crate::clock::awake_since)
}

/// Seconds since `s` on the wall clock, sleep included: for "in the last day" windows.
pub fn wall_age_secs(s: Option<&str>) -> Option<f64> {
    s.and_then(parse_iso).map(|t| now_ts() - t)
}

pub fn local_dt(ts: f64) -> DateTime<Local> {
    Local.timestamp_opt(ts.floor() as i64, 0).single().unwrap_or_else(Local::now)
}

pub fn local_now() -> DateTime<Local> {
    local_dt(now_ts())
}

pub fn local_hhmm(s: Option<&str>) -> String {
    let ts = match s {
        Some(s) => match parse_iso(s) {
            Some(t) => t,
            None => return String::new(),
        },
        None => now_ts(),
    };
    local_dt(ts).format("%H:%M").to_string()
}

pub fn twelve_hour(hhmm: &str) -> String {
    let mut it = hhmm.split(':');
    let h: u32 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let m: u32 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let hh = if h % 12 == 0 { 12 } else { h % 12 };
    let mm = if m > 0 { format!(":{m:02}") } else { String::new() };
    format!("{hh}{mm}{}", if h < 12 { "am" } else { "pm" })
}

pub fn local_clock(s: Option<&str>) -> String {
    let hhmm = local_hhmm(s);
    if hhmm.is_empty() {
        hhmm
    } else {
        twelve_hour(&hhmm)
    }
}

/// Minutes in a length of time an agent wrote: `3h`, `90m`, `1h30m`, `1.5h`, `2 hours`, or a bare
/// number of minutes. `None` when it isn't one, or is zero or more than a month.
pub fn parse_minutes(text: &str) -> Option<i64> {
    static PART: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^\s*(\d+(?:\.\d+)?)\s*(days?|d|hours?|hrs?|h|minutes?|mins?|m)?\s*").unwrap());
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }
    let mut total = 0.0;
    while !rest.is_empty() {
        let c = PART.captures(rest)?;
        let n: f64 = c[1].parse().ok()?;
        let unit = c.get(2).map(|u| u.as_str().to_lowercase()).unwrap_or_default();
        total += n * match unit.chars().next() {
            Some('d') => 8.0 * 60.0,
            Some('h') => 60.0,
            _ => 1.0,
        };
        rest = &rest[c[0].len()..];
    }
    let m = total.round() as i64;
    (m > 0 && m <= 31 * 24 * 60).then_some(m)
}

pub fn prefix(kind: &str) -> &'static str {
    match kind {
        "task" => "T",
        "goal" => "G",
        "issue" => "B",
        "job" => "J",
        _ => "?",
    }
}

pub fn rf(kind: &str, id: i64) -> String {
    format!("{}{}", prefix(kind), id)
}

pub fn rf_opt(kind: &str, id: Option<i64>) -> Value {
    id.map(|i| Value::String(rf(kind, i))).unwrap_or(Value::Null)
}

static REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([TtGgBbJj])?(\d+)$").unwrap());

pub fn parse_ref(v: &Value, kind: &str) -> Result<Option<i64>> {
    match v {
        Value::Null => Ok(None),
        Value::Bool(_) => err(400, "That id isn't valid."),
        Value::Number(n) => Ok(n.as_i64()),
        Value::String(s) => parse_ref_str(s, kind),
        _ => err(400, "That id isn't valid."),
    }
}

pub fn parse_ref_str(s: &str, kind: &str) -> Result<Option<i64>> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let Some(c) = REF_RE.captures(s) else {
        return err(400, format!("“{s}” isn't a valid id."));
    };
    if let Some(p) = c.get(1) {
        if !kind.is_empty() && p.as_str().to_uppercase() != prefix(kind) {
            return err(400, format!("“{s}” isn't a {kind} id."));
        }
    }
    Ok(c[2].parse().ok())
}

pub fn need_ref(v: &Value, kind: &str) -> Result<i64> {
    parse_ref(v, kind)?.ok_or_else(|| ApiError::new(400, format!("Say which {kind}.")))
}

pub fn jloads_obj(s: Option<&str>) -> Row {
    match s.and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        Some(Value::Object(m)) => m,
        _ => Row::new(),
    }
}

pub fn jloads_arr(s: Option<&str>) -> Vec<Value> {
    match s.and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    }
}

pub fn jdumps(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

pub fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

pub fn one_line(text: &str, limit: usize) -> String {
    clip(&text.split_whitespace().collect::<Vec<_>>().join(" "), limit)
}

static SENTENCE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(.+?[.!?])(\s|$)").unwrap());

pub fn first_sentence(text: &str, limit: usize) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return t;
    }
    let s = SENTENCE_RE.captures(&t).map(|c| c[1].to_string()).unwrap_or(t);
    clip(&s, limit)
}

static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-z0-9]+").unwrap());

pub fn norm_title(title: &str) -> String {
    let t = title.to_lowercase().replace('’', "'");
    NON_ALNUM.replace_all(&t, " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn as_bool(v: Option<&Value>, default: bool) -> bool {
    match v {
        None | Some(Value::Null) => default,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(default),
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Some(_) => default,
    }
}

pub fn str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        None | Some(Value::Null) => vec![],
        Some(Value::String(s)) => {
            if s.trim().is_empty() {
                vec![]
            } else {
                vec![s.clone()]
            }
        }
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .filter(|s| !s.trim().is_empty())
            .collect(),
        Some(other) => vec![other.to_string()],
    }
}

pub fn plural(n: i64, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

pub fn short(text: &str, n: usize) -> String {
    clip(&one_line(text, 400), n)
}

pub fn base_name(path: &str) -> Option<String> {
    let p = path.trim_end_matches('/');
    let b = p.rsplit('/').next().unwrap_or("");
    if b.is_empty() {
        None
    } else {
        Some(b.to_string())
    }
}

pub fn expand_home(p: &str) -> std::path::PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(h) = dirs::home_dir() {
            return h.join(rest);
        }
    }
    if p == "~" {
        if let Some(h) = dirs::home_dir() {
            return h;
        }
    }
    std::path::PathBuf::from(p)
}

/// A pull request found in text: GitHub, GitLab or Bitbucket.
#[derive(Debug, Clone, PartialEq)]
pub struct PrLink {
    pub host: String,
    pub repo: String,
    pub num: i64,
    pub url: String,
}

static PR_RES: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        ("github", Regex::new(r"https?://(?:www\.)?github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/pull/(\d+)").unwrap()),
        ("gitlab", Regex::new(r"https?://[A-Za-z0-9.-]*gitlab[A-Za-z0-9.-]*/([A-Za-z0-9_./-]+?)/-/merge_requests/(\d+)").unwrap()),
        ("bitbucket", Regex::new(r"https?://(?:www\.)?bitbucket\.org/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/pull-requests/(\d+)").unwrap()),
    ]
});

pub fn find_pr(text: &str) -> Option<PrLink> {
    let mut best: Option<(usize, PrLink)> = None;
    for (host, re) in PR_RES.iter() {
        if let Some(c) = re.captures(text) {
            let at = c.get(0).unwrap().start();
            let link = PrLink {
                host: host.to_string(),
                repo: c[1].to_string(),
                num: c[2].parse().unwrap_or(0),
                url: c.get(0).unwrap().as_str().to_string(),
            };
            if best.as_ref().map(|(b, _)| at < *b).unwrap_or(true) {
                best = Some((at, link));
            }
        }
    }
    best.map(|(_, l)| l)
}

/// Accessors for a database row held as a JSON object.
pub trait RowExt {
    fn s(&self, k: &str) -> Option<&str>;
    fn st(&self, k: &str) -> String {
        self.s(k).unwrap_or("").to_string()
    }
    fn i(&self, k: &str) -> Option<i64>;
    fn i0(&self, k: &str) -> i64 {
        self.i(k).unwrap_or(0)
    }
    fn f(&self, k: &str) -> Option<f64>;
    fn b(&self, k: &str) -> bool {
        self.i(k).map(|v| v != 0).unwrap_or(false)
    }
    fn id(&self) -> i64 {
        self.i0("id")
    }
    fn v(&self, k: &str) -> Value;
}

impl RowExt for Row {
    fn s(&self, k: &str) -> Option<&str> {
        match self.get(k) {
            Some(Value::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }
    fn i(&self, k: &str) -> Option<i64> {
        match self.get(k) {
            Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Some(Value::Bool(b)) => Some(*b as i64),
            Some(Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        }
    }
    fn f(&self, k: &str) -> Option<f64> {
        match self.get(k) {
            Some(Value::Number(n)) => n.as_f64(),
            _ => None,
        }
    }
    fn v(&self, k: &str) -> Value {
        self.get(k).cloned().unwrap_or(Value::Null)
    }
}

/// Like Python's truthiness for an optional string: present and non-empty.
pub fn has(s: Option<&str>) -> bool {
    s.map(|x| !x.is_empty()).unwrap_or(false)
}

pub fn opt_str(v: Option<&str>) -> Value {
    match v {
        Some(s) if !s.is_empty() => Value::String(s.to_string()),
        _ => Value::Null,
    }
}

/// Body helpers for JSON request bodies.
pub fn body_str(body: &Value, key: &str) -> String {
    match body.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.trim().to_string(),
        Some(other) => other.to_string().trim().to_string(),
    }
}

pub fn body_has(body: &Value, key: &str) -> bool {
    match body.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refs_round_trip() {
        assert_eq!(parse_ref(&json!("T12"), "task").unwrap(), Some(12));
        assert_eq!(parse_ref(&json!("12"), "task").unwrap(), Some(12));
        assert_eq!(parse_ref(&json!(7), "goal").unwrap(), Some(7));
        assert!(parse_ref(&json!("G3"), "task").is_err());
        assert_eq!(rf("issue", 4), "B4");
    }

    #[test]
    fn clipping() {
        assert_eq!(clip("hello world", 6), "hello…");
        assert_eq!(one_line("a\n  b", 10), "a b");
        assert_eq!(first_sentence("One. Two.", 200), "One.");
        assert_eq!(norm_title("Fix the  Thing’s bug!"), "fix the thing s bug");
    }

    #[test]
    fn finds_prs_on_any_host() {
        let p = find_pr("see https://github.com/acme/web/pull/42 now").unwrap();
        assert_eq!((p.host.as_str(), p.repo.as_str(), p.num), ("github", "acme/web", 42));
        let p = find_pr("https://bitbucket.org/team/api/pull-requests/9").unwrap();
        assert_eq!((p.host.as_str(), p.num), ("bitbucket", 9));
        let p = find_pr("https://gitlab.com/group/sub/proj/-/merge_requests/3").unwrap();
        assert_eq!((p.repo.as_str(), p.num), ("group/sub/proj", 3));
        assert!(find_pr("no link").is_none());
    }

    #[test]
    fn clock_text() {
        assert_eq!(twelve_hour("15:00"), "3pm");
        assert_eq!(twelve_hour("06:30"), "6:30am");
        assert_eq!(twelve_hour("00:00"), "12am");
    }

    #[test]
    fn minutes() {
        assert_eq!(parse_minutes("3h"), Some(180));
        assert_eq!(parse_minutes("90m"), Some(90));
        assert_eq!(parse_minutes("1h30m"), Some(90));
        assert_eq!(parse_minutes("1.5h"), Some(90));
        assert_eq!(parse_minutes("2 hours"), Some(120));
        assert_eq!(parse_minutes("45"), Some(45));
        assert_eq!(parse_minutes("1d"), Some(480));
        assert_eq!(parse_minutes("soon"), None);
        assert_eq!(parse_minutes("0h"), None);
    }

    #[test]
    fn iso_parse() {
        let t = parse_iso("2026-09-29T21:30:00Z").unwrap();
        assert_eq!(iso(t), "2026-09-29T21:30:00Z");
    }
}
