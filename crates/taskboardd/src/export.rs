//! The debug export: one zip the owner can send to anybody so the board can be debugged on another
//! machine (`tb export`, and the app's File → Export for Debugging…). It holds a manifest and README
//! (versions, macOS and hardware, install and channel, the daemon's state and uptime, when it was made),
//! every table of the board, the daemon and hook logs, and the state files around them.
//!
//! Everything in it is redacted, and the redaction keeps what's safe rather than strips what's known:
//! ids, numbers, timestamps, board refs (`T12`, `G3`), UUIDs and the lowercase values of status-like
//! columns (`status`, `kind`, `pr_state`, …) stay; every other string becomes a placeholder such as
//! `<text:1a2b3c4d>`, `<path:…>` or `<url:…>`. A placeholder is a salted hash of the value, so the same
//! value gets the same placeholder everywhere in one export (a project name in `tasks.json`, in
//! `goals.json` and in `server.log` still line up), and the salt is never written, so placeholders
//! can't be reversed by guessing. Logs are free text: every value the board's tables redacted is
//! replaced where it appears, then paths, URLs, emails, tokens, Jira keys, branch names, quoted text
//! and long opaque strings are.
//!
//! The board is read from `tasks.db` opened read-only, so an export works with the daemon down. It
//! never reads the app's token or socket, the attachments in `evidence/`, the task copies in `tasks/`,
//! the Claude transcripts or the GitHub sign-in folder; it only lists their sizes.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use aho_corasick::{AhoCorasick, MatchKind};
use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};

use crate::config::Config;

/// The bundle's layout version, in the manifest.
const FORMAT: u32 = 1;
/// Rows kept per table: the newest ones.
const MAX_ROWS: usize = 20_000;
/// Bytes kept from the end of each log.
const MAX_LOG: u64 = 4 * 1024 * 1024;
/// Files kept from each folder of small state files (spool, statusline).
const MAX_STATE_FILES: usize = 100;
/// Entries listed in the data folder's inventory.
const MAX_INVENTORY: usize = 5_000;

/// Columns and JSON keys whose short lowercase values are the board's own words (`working`, `merged`,
/// `design`), not the owner's, so they're kept.
const ENUM_KEYS: &[&str] = &[
    "status", "state", "kind", "priority", "pickup", "verdict", "pr_state", "pr_build", "pr_review", "pr_phase", "pr_flow",
    "pr_host", "needs_reason", "agent", "api_error_kind", "impact", "origin", "closed_by", "purpose", "type", "phase", "mode",
    "provider", "event", "key", "who", "by", "source", "level", "result", "outcome", "reason", "action", "role", "host_kind",
];
/// Jira's statuses are words with capitals and spaces ("In Progress").
const STATUS_KEYS: &[&str] = &["jira_status", "epic_status"];
/// JSON objects whose keys are the owner's names (projects, hosts), not field names.
const USER_KEYED: &[&str] = &["projects", "hosts", "repos", "paths", "env", "accounts", "by_project"];
/// Path and URL parts too common to say anything about the owner.
const COMMON_PARTS: &[&str] = &[
    "users", "home", "private", "volumes", "development", "documents", "desktop", "downloads", "library", "applications",
    "http", "https", "github.com", "www.github.com", "bitbucket.org", "atlassian.net", "pull", "pulls", "browse", "issues",
    "projects", "repos", "tasks", "main", "master", "src", "crates", "target", "debug", "release",
];
/// Data-folder entries never opened, not even listed: they're only for the app.
const NEVER: &[&str] = &[crate::apptoken::FILE, crate::apporigin::SOCKET];

static SAFE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?i)^(?:",
        r"\d{4}-\d{2}-\d{2}(?:[T ]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?)?(?:Z|[+-]\d{2}:?\d{2})?",
        r"|\d{1,2}(?::\d{2}){1,2}(?:\s?[ap]m)?|\d{1,2}\s?[ap]m",
        r"|[-+]?\d+(?:\.\d+)?(?:e[-+]?\d+)?",
        r"|\d+(?:\.\d+)?\s?(?:ms|s|m|h|d|w|mins?|secs?|hours?|days?)",
        r"|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
        r"|[TGBMJQFWS]\d+(?:\s*[,;]\s*[TGBMJQFWS]\d+)*",
        r"|true|false|on|off|yes|no|null|none|default|auto",
        r"|(?:mon|tue|wed|thu|fri|sat|sun)[a-z]*(?:-(?:mon|tue|wed|thu|fri|sat|sun)[a-z]*)?",
        r")$"
    ))
    .unwrap()
});
static ENUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-z][a-z0-9_.-]{0,39}$").unwrap());
static STATUS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z][A-Za-z ]{0,30}$").unwrap());
static FIELD: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-z_][a-z0-9_]{0,40}$").unwrap());
static FILE_NAME: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9_.-]{1,64}$").unwrap());
static EMAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[^\s@]+@[^\s@]+\.[A-Za-z]{2,}$").unwrap());
static URL: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*://").unwrap());
static PART_SPLIT: Lazy<Regex> = Lazy::new(|| Regex::new(r"[/:@#?=&]+").unwrap());

/// What free text is scrubbed of, in order, after the board's own values: each match is replaced as a
/// whole with a placeholder of its kind.
static TEXT_RULES: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        ("secret", Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[abposr]-[A-Za-z0-9-]{10,}|sk-[A-Za-z0-9_-]{20,}|ATATT[A-Za-z0-9_=-]{20,}|glpat-[A-Za-z0-9_-]{20,}|AKIA[A-Z0-9]{16})").unwrap()),
        ("secret", Regex::new(r#"(?i)\b(?:bearer|token|password|passwd|secret|authorization|api[_-]?key)\b["']?\s*[:=]?\s*["']?[A-Za-z0-9._~+/=-]{8,}"#).unwrap()),
        ("url", Regex::new(r#"[A-Za-z][A-Za-z0-9+.-]*://[^\s"'<>`)\]]+"#).unwrap()),
        ("url", Regex::new(r"\b[\w.-]+@[\w.-]+:[\w./~-]+").unwrap()),
        ("email", Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").unwrap()),
        ("path", Regex::new(r"(?:~|\B)/[A-Za-z0-9._@+-]+(?:/[A-Za-z0-9._@+-]*)+").unwrap()),
        ("text", Regex::new(r"\b(?:feat|feature|fix|bugfix|hotfix|chore|release|refactor|docs|test)/[\w./-]+").unwrap()),
        ("text", Regex::new(r"\b[\w.-]+/[\w.-]+#\d+").unwrap()),
        ("text", Regex::new(r"\b[A-Z][A-Z0-9]{1,9}-\d+\b").unwrap()),
        ("text", Regex::new(r#""[^"\n]*"|`[^`\n]*`"#).unwrap()),
        ("id", Regex::new(r"[A-Za-z0-9+/=_-]{32,}").unwrap()),
    ]
});

/// Who and where the export runs, for the redactor's list of values that are always hidden.
#[derive(Default, Clone)]
pub struct Sources {
    pub home: String,
    pub user: String,
    pub hosts: Vec<String>,
    pub names: Vec<String>,
    pub emails: Vec<String>,
    /// Values of environment variables named like secrets (`*TOKEN*`, `*KEY*`, …).
    pub secrets: Vec<String>,
}

impl Sources {
    /// This machine's: the home folder, user, host names, git's name and email, and secret-looking
    /// environment values.
    pub fn here() -> Sources {
        let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default();
        let user = std::env::var("USER").unwrap_or_default();
        let mut hosts = vec![hostname()];
        for k in ["ComputerName", "LocalHostName", "HostName"] {
            hosts.push(output("scutil", &["--get", k]));
        }
        let names = vec![output("id", &["-F"]), output("git", &["config", "--global", "--get", "user.name"])];
        let emails = vec![output("git", &["config", "--global", "--get", "user.email"])];
        let secrets = std::env::vars().filter(|(k, v)| secret_name(k) && v.len() >= 8).map(|(_, v)| v).collect();
        Sources { home, user, hosts, names, emails, secrets }
    }
}

fn secret_name(k: &str) -> bool {
    let k = k.to_ascii_uppercase();
    ["TOKEN", "SECRET", "KEY", "PASS", "AUTH", "CREDENTIAL", "COOKIE", "SESSION"].iter().any(|w| k.contains(w))
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer outlives the call and its length is passed with it.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    if !ok {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).to_string()
}

fn output(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

// ------------------------------------------------------------------ redactor

/// Turns values into placeholders, remembering each so free text can be scrubbed of them later.
pub struct Redactor {
    salt: [u64; 2],
    /// Value → its placeholder, for every value hidden so far.
    seen: HashMap<String, String>,
    /// Values hidden wherever they appear, even in a status column: the owner, the user, hosts, secrets.
    always: HashMap<String, String>,
}

/// The board's hidden values, as one matcher over free text.
pub struct Index {
    ac: Option<AhoCorasick>,
    placeholders: Vec<String>,
}

impl Redactor {
    pub fn new(salt: [u64; 2], src: &Sources, cfg: &Config) -> Redactor {
        let mut r = Redactor { salt, seen: HashMap::new(), always: HashMap::new() };
        let add = |r: &mut Redactor, kind: &str, v: &str| {
            let v = v.trim();
            if v.len() >= 3 && !(kind != "secret" && SAFE.is_match(v)) {
                let p = r.placeholder(kind, v);
                r.always.insert(v.to_lowercase(), p);
            }
        };
        add(&mut r, "path", &src.home);
        add(&mut r, "user", &src.user);
        for h in &src.hosts {
            add(&mut r, "host", h);
            if let Some(short) = h.strip_suffix(".local") {
                add(&mut r, "host", short);
            }
        }
        for n in src.names.iter().chain(std::iter::once(&cfg.owner).filter(|o| o.as_str() != "the owner")) {
            add(&mut r, "user", n);
        }
        for e in src.emails.iter().chain(&cfg.owner_emails) {
            add(&mut r, "email", e);
        }
        for s in &src.secrets {
            add(&mut r, "secret", s);
        }
        r
    }

    fn placeholder(&self, kind: &str, s: &str) -> String {
        let mut h = DefaultHasher::new();
        self.salt.hash(&mut h);
        s.hash(&mut h);
        format!("<{kind}:{:08x}>", h.finish() as u32)
    }

    fn always(&self, s: &str) -> Option<&String> {
        self.always.get(&s.trim().to_lowercase())
    }

    /// Whether a string is safe to keep as it is, under a column or JSON key.
    fn keeps(&self, key: Option<&str>, s: &str) -> bool {
        if s.trim().is_empty() {
            return true;
        }
        if self.always(s).is_some() {
            return false;
        }
        if SAFE.is_match(s.trim()) {
            return true;
        }
        match key {
            Some(k) if ENUM_KEYS.contains(&k) => ENUM.is_match(s),
            Some(k) if STATUS_KEYS.contains(&k) => STATUS.is_match(s),
            _ => false,
        }
    }

    /// The placeholder for a value, remembered (with its path and URL parts) for scrubbing free text.
    pub fn hide(&mut self, s: &str) -> String {
        let kind = if URL.is_match(s) {
            "url"
        } else if EMAIL.is_match(s) {
            "email"
        } else if s.starts_with('/') || s.starts_with("~/") {
            "path"
        } else {
            "text"
        };
        self.hide_as(kind, s)
    }

    fn hide_as(&mut self, kind: &str, s: &str) -> String {
        if let Some(p) = self.always(s) {
            return p.clone();
        }
        if let Some(p) = self.seen.get(s) {
            return p.clone();
        }
        let p = self.placeholder(kind, s);
        if s.len() >= 4 {
            self.seen.insert(s.to_string(), p.clone());
        }
        if !s.contains(char::is_whitespace) {
            let parts: Vec<String> = PART_SPLIT.split(s).map(|p| p.trim_end_matches(".git").to_string()).collect();
            for part in parts {
                let common = COMMON_PARTS.contains(&part.to_lowercase().as_str());
                if part.len() >= 4 && part != s && !common && !SAFE.is_match(&part) && !self.seen.contains_key(&part) && self.always(&part).is_none() {
                    let pp = self.placeholder("text", &part);
                    self.seen.insert(part, pp);
                }
            }
        }
        p
    }

    /// A string from a column or JSON field: kept, hidden, or (when it holds JSON) redacted as JSON.
    pub fn string(&mut self, key: Option<&str>, s: &str) -> Value {
        let t = s.trim();
        if (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']')) {
            if let Ok(v @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(t) {
                return self.value(key, &v);
            }
        }
        if self.keeps(key, s) {
            return Value::String(s.to_string());
        }
        Value::String(self.hide(s))
    }

    /// A JSON value: field names kept when they look like field names, every string judged by its key.
    pub fn value(&mut self, key: Option<&str>, v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let owner_named = key.is_some_and(|k| USER_KEYED.contains(&k));
                let mut out = Map::new();
                for (k, x) in m {
                    let name = if !owner_named && FIELD.is_match(k) && self.always(k).is_none() { k.clone() } else { self.hide(k) };
                    let x = self.value(Some(k), x);
                    out.insert(name, x);
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(key, x)).collect()),
            Value::String(s) => self.string(key, s),
            other => other.clone(),
        }
    }

    /// Everything hidden so far, as one matcher for [`Redactor::text`].
    pub fn index(&self) -> Index {
        let mut pairs: Vec<(&String, &String)> = self.always.iter().collect();
        pairs.extend(self.seen.iter().filter(|(k, _)| k.len() <= 1000 && self.always(k).is_none()));
        let ac = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .match_kind(MatchKind::LeftmostLongest)
            .build(pairs.iter().map(|(k, _)| k.as_str()))
            .ok();
        Index { ac, placeholders: pairs.into_iter().map(|(_, p)| p.clone()).collect() }
    }

    /// Free text (a log, a file that isn't JSON) with every hidden value and everything that looks
    /// like a path, URL, email, token, key, branch, quoted text or opaque string replaced.
    pub fn text(&mut self, index: &Index, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut at = 0;
        if let Some(ac) = &index.ac {
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            for m in ac.find_iter(s) {
                let hit = &s[m.start()..m.end()];
                let bounded_left = !word(hit.chars().next()) || !word(s[..m.start()].chars().next_back());
                let bounded_right = !word(hit.chars().next_back()) || !word(s[m.end()..].chars().next());
                if !(bounded_left && bounded_right) {
                    continue;
                }
                out.push_str(&s[at..m.start()]);
                out.push_str(&index.placeholders[m.pattern().as_usize()]);
                at = m.end();
            }
        }
        out.push_str(&s[at..]);
        let mut s = out;
        for (kind, re) in TEXT_RULES.iter() {
            let from = s.clone();
            s = re
                .replace_all(&from, |c: &regex::Captures| {
                    let m = c.get(0).unwrap();
                    let in_placeholder = from[..m.start()].ends_with('<');
                    if in_placeholder { m.as_str().to_string() } else { self.text_match(kind, m.as_str()) }
                })
                .into_owned();
        }
        s
    }

    fn text_match(&mut self, kind: &str, m: &str) -> String {
        let local = ["http://127.0.0.1", "http://localhost", "https://127.0.0.1", "https://localhost"];
        if kind == "url" && local.iter().any(|l| m.starts_with(l)) {
            return m.to_string();
        }
        if kind == "path" && m.starts_with("/tasks/") {
            return m.to_string();
        }
        if kind == "id" && SAFE.is_match(m) {
            return m.to_string();
        }
        if m.starts_with('"') || m.starts_with('`') {
            let q = &m[..1];
            let inner = &m[1..m.len() - 1];
            let placeholder_only = inner.starts_with('<') && inner.ends_with('>') && !inner[1..].contains('<');
            if placeholder_only || self.keeps(None, inner) {
                return m.to_string();
            }
            return format!("{q}{}{q}", self.hide(inner));
        }
        self.hide_as(kind, m)
    }

    /// How many distinct values were hidden, by kind.
    fn counts(&self) -> BTreeMap<String, usize> {
        let mut all: Vec<&String> = self.seen.values().chain(self.always.values()).collect();
        all.sort();
        all.dedup();
        let mut out = BTreeMap::new();
        for p in all {
            let kind = p.trim_start_matches('<').split(':').next().unwrap_or("text").to_string();
            *out.entry(kind).or_insert(0) += 1;
        }
        out
    }
}

// ------------------------------------------------------------------ bundle

/// Writes the export into `out_dir` and returns its path. `made_by` says who asked (`tb`, `app`).
pub fn run(cfg: &Config, made_by: &str, out_dir: &Path) -> Result<PathBuf, String> {
    let now = chrono::Local::now();
    let entries = bundle(cfg, made_by, rand::random(), &Sources::here(), daemon(cfg), now)?;
    let bytes = zip(&entries, now.naive_local())?;
    std::fs::create_dir_all(out_dir).map_err(|e| format!("couldn't make {}: {e}", out_dir.display()))?;
    let stem = format!("taskboard-debug-{}", now.format("%Y-%m-%d-%-I.%M.%S%P"));
    let mut path = out_dir.join(format!("{stem}.zip"));
    let mut n = 2;
    while path.exists() {
        path = out_dir.join(format!("{stem}-{n}.zip"));
        n += 1;
    }
    let tmp = path.with_extension("zip.partial");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("couldn't write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    Ok(path)
}

/// Where an export goes when nobody says: Downloads, else the current folder.
pub fn default_dir() -> PathBuf {
    dirs::download_dir().filter(|d| d.is_dir()).or_else(|| std::env::current_dir().ok()).unwrap_or_else(|| PathBuf::from("."))
}

/// The daemon's answer at its root: whether it's running, its version and uptime.
pub fn daemon(cfg: &Config) -> Value {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(2)).build();
    let root = format!("{}/tasks", cfg.url().trim_end_matches('/'));
    let answer = agent.get(&root).call().ok().and_then(|r| r.into_string().ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok());
    match answer {
        Some(v) if v["board"] == "taskboardd" => json!({"running": true, "version": v["version"], "uptime_secs": v["uptime_secs"]}),
        _ => json!({"running": false}),
    }
}

/// The zip's files, in order: `(name, contents)`.
pub fn bundle(cfg: &Config, made_by: &str, salt: [u64; 2], src: &Sources, daemon: Value, now: chrono::DateTime<chrono::Local>) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut r = Redactor::new(salt, src, cfg);
    let mut entries: Vec<(String, Vec<u8>)> = vec![];

    let (schema, tables) = board(cfg, &mut r, &mut entries)?;
    if !schema.is_empty() {
        entries.push(("board/schema.sql".into(), schema.into_bytes()));
    }
    state(cfg, &mut r, &mut entries);
    let index = r.index();
    logs(cfg, &mut r, &index, &mut entries);
    let inventory = inventory(cfg, &mut r);
    entries.push(("state/files.json".into(), pretty(&inventory)));

    let facts = facts(&src.home);
    let manifest = json!({
        "format": FORMAT,
        "made_at": now.to_rfc3339(),
        "made_by": made_by,
        "version": env!("CARGO_PKG_VERSION"),
        "channel": if env!("CARGO_PKG_VERSION").contains("beta") { "beta" } else { "stable" },
        "daemon": daemon,
        "system": facts,
        "tables": tables,
        "redacted": r.counts(),
        "files": entries.iter().map(|(n, b)| json!({"name": n, "bytes": b.len()})).collect::<Vec<_>>(),
    });
    let readme = readme(&manifest, now);
    entries.insert(0, ("manifest.json".into(), pretty(&manifest)));
    entries.insert(0, ("README.md".into(), readme.into_bytes()));
    Ok(entries)
}

fn pretty(v: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(v).unwrap_or_default()
}

/// Every table, newest rows first capped at [`MAX_ROWS`], redacted. Returns the schema and each
/// table's `{total, kept}`.
fn board(cfg: &Config, r: &mut Redactor, entries: &mut Vec<(String, Vec<u8>)>) -> Result<(String, Value), String> {
    let path = cfg.db_path();
    if !path.is_file() {
        return Ok((String::new(), json!({})));
    }
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| format!("couldn't open the board: {e}"))?;
    conn.busy_timeout(Duration::from_secs(10)).ok();
    let fail = |e: rusqlite::Error| format!("couldn't read the board: {e}");
    let mut stmt = conn.prepare("SELECT type, name, sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type DESC, name").map_err(fail)?;
    let rows: Vec<(String, String, String)> =
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(fail)?.filter_map(|r| r.ok()).collect();
    let schema = rows.iter().map(|(_, _, sql)| format!("{sql};\n")).collect::<String>();
    let mut counts = Map::new();
    for (_, name, _) in rows.iter().filter(|(t, _, _)| t == "table") {
        let quoted = format!("\"{}\"", name.replace('"', "\"\""));
        let total: i64 = conn.query_row(&format!("SELECT count(*) FROM {quoted}"), [], |row| row.get(0)).unwrap_or(0);
        let newest = format!("SELECT * FROM {quoted} ORDER BY rowid DESC LIMIT {MAX_ROWS}");
        let mut stmt = match conn.prepare(&newest).or_else(|_| conn.prepare(&format!("SELECT * FROM {quoted} LIMIT {MAX_ROWS}"))) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let columns: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
        let mut raw = vec![];
        let mut q = stmt.query([]).map_err(fail)?;
        while let Some(row) = q.next().map_err(fail)? {
            let mut cells = vec![];
            for i in 0..columns.len() {
                cells.push(match row.get_ref(i).map_err(fail)? {
                    ValueRef::Null => Cell::Null,
                    ValueRef::Integer(n) => Cell::Value(json!(n)),
                    ValueRef::Real(f) => Cell::Value(json!(f)),
                    ValueRef::Text(t) => Cell::Text(String::from_utf8_lossy(t).into_owned()),
                    ValueRef::Blob(b) => Cell::Value(json!(format!("<blob:{} bytes>", b.len()))),
                });
            }
            raw.push(cells);
        }
        raw.reverse();
        let rows: Vec<Value> = raw
            .into_iter()
            .map(|cells| {
                let mut obj = Map::new();
                for (c, cell) in columns.iter().zip(cells) {
                    let v = match cell {
                        Cell::Null => Value::Null,
                        Cell::Value(v) => v,
                        Cell::Text(t) => r.string(Some(c), &t),
                    };
                    obj.insert(c.clone(), v);
                }
                Value::Object(obj)
            })
            .collect();
        let kept = rows.len();
        counts.insert(name.clone(), json!({"total": total, "kept": kept}));
        let file_name = if FILE_NAME.is_match(name) { name.clone() } else { r.hide(name) };
        entries.push((format!("board/{file_name}.json"), pretty(&json!({"table": file_name, "total": total, "kept": kept, "rows": rows}))));
    }
    Ok((schema, Value::Object(counts)))
}

enum Cell {
    Null,
    Value(Value),
    Text(String),
}

/// The state files beside the board: config, hooks, accounts (who, never tokens), the spool, the
/// statusline snapshots, and any other JSON in the data folder.
fn state(cfg: &Config, r: &mut Redactor, entries: &mut Vec<(String, Vec<u8>)>) {
    if let Ok(t) = std::fs::read_to_string(&cfg.config_path) {
        let v = toml::from_str::<toml::Value>(&t).ok().and_then(|v| serde_json::to_value(v).ok());
        match v {
            Some(v) => entries.push(("state/config.json".into(), pretty(&r.value(None, &v)))),
            None => {
                let index = r.index();
                entries.push(("state/config.toml".into(), r.text(&index, &t).into_bytes()));
            }
        }
    }
    let hooks = crate::hooks::path(cfg);
    json_file(r, &hooks, "state/hooks.json", entries);
    let Ok(dir) = std::fs::read_dir(&cfg.data) else { return };
    let mut top: Vec<PathBuf> = dir.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    top.sort();
    for p in top {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if NEVER.contains(&name.as_str()) || p == hooks || !name.ends_with(".json") || !FILE_NAME.is_match(&name) {
            continue;
        }
        json_file(r, &p, &format!("state/{name}"), entries);
    }
    for (dir, as_) in [(cfg.spool_dir(), "spool"), (cfg.statusline_dir.clone(), "statusline")] {
        for (i, p) in newest_files(&dir, MAX_STATE_FILES).into_iter().enumerate() {
            json_file(r, &p, &format!("state/{as_}/{}.json", i + 1), entries);
        }
    }
}

fn json_file(r: &mut Redactor, path: &Path, as_: &str, entries: &mut Vec<(String, Vec<u8>)>) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if !meta.is_file() || meta.len() > 8 * 1024 * 1024 {
        return;
    }
    let Ok(t) = std::fs::read_to_string(path) else { return };
    let v = match serde_json::from_str::<Value>(&t) {
        Ok(v) => r.value(None, &v),
        Err(_) => json!({"unreadable": true, "bytes": t.len()}),
    };
    entries.push((as_.to_string(), pretty(&v)));
}

/// A folder's newest regular files, up to `max`.
fn newest_files(dir: &Path, max: usize) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let m = std::fs::symlink_metadata(e.path()).ok()?;
            m.is_file().then(|| (m.modified().unwrap_or(std::time::UNIX_EPOCH), e.path()))
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().take(max).map(|(_, p)| p).collect()
}

/// The daemon's and hooks' logs (every `*.log*` in the data folder), the last [`MAX_LOG`] bytes of each,
/// scrubbed.
fn logs(cfg: &Config, r: &mut Redactor, index: &Index, entries: &mut Vec<(String, Vec<u8>)>) {
    let Ok(dir) = std::fs::read_dir(&cfg.data) else { return };
    let mut names: Vec<String> = dir.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.contains(".log") && FILE_NAME.is_match(n)).collect();
    names.sort();
    for name in names {
        let path = cfg.data.join(&name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(text) = tail(&path, MAX_LOG) else { continue };
        entries.push((format!("logs/{name}"), r.text(index, &text).into_bytes()));
    }
}

/// The end of a file, from the first whole line.
fn tail(path: &Path, max: u64) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(max);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = vec![];
    f.read_to_end(&mut buf).ok()?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        if let Some(nl) = text.find('\n') {
            text.drain(..=nl);
        }
    }
    Some(text)
}

/// The data folder's files: names (hidden below the top level), sizes and times; never contents.
fn inventory(cfg: &Config, r: &mut Redactor) -> Value {
    let mut out = vec![];
    walk(&cfg.data, "", 1, r, &mut out);
    json!({"files": out, "truncated": out.len() >= MAX_INVENTORY})
}

fn walk(dir: &Path, prefix: &str, depth: usize, r: &mut Redactor, out: &mut Vec<Value>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut items: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    items.sort_by_key(|e| e.file_name());
    for e in items {
        if out.len() >= MAX_INVENTORY {
            return;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if depth == 1 && NEVER.contains(&name.as_str()) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        let shown = if depth == 1 && FILE_NAME.is_match(&name) && r.always(&name).is_none() {
            name.clone()
        } else {
            let ext = Path::new(&name).extension().map(|x| x.to_string_lossy().to_string()).filter(|x| x.len() <= 5 && x.chars().all(|c| c.is_ascii_alphanumeric()));
            let p = r.placeholder("file", &name);
            ext.map(|x| format!("{p}.{x}")).unwrap_or(p)
        };
        let path = format!("{prefix}{shown}");
        let modified = meta.modified().ok().map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339());
        let kind = if meta.is_dir() { "dir" } else if meta.file_type().is_symlink() { "link" } else { "file" };
        out.push(json!({"path": path, "kind": kind, "bytes": meta.len(), "modified": modified}));
        if meta.is_dir() && depth < 3 {
            walk(&e.path(), &format!("{path}/"), depth + 1, r, out);
        }
    }
}

// ------------------------------------------------------------------ manifest

/// macOS, hardware, architecture and how this binary was installed.
fn facts(home: &str) -> Value {
    let sysctl = |k: &str| output("sysctl", &["-n", k]);
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let memory_gb = sysctl("hw.memsize").parse::<u64>().ok().map(|b| b >> 30);
    json!({
        "macos": output("sw_vers", &["-productVersion"]),
        "macos_build": output("sw_vers", &["-buildVersion"]),
        "model": sysctl("hw.model"),
        "cpu": sysctl("machdep.cpu.brand_string"),
        "memory_gb": memory_gb,
        "arch": std::env::consts::ARCH,
        "rosetta": sysctl("sysctl.proc_translated") == "1",
        "install": install_kind(&exe, home),
    })
}

/// Where the binary runs from, without the home folder's name.
fn install_kind(exe: &str, home: &str) -> String {
    if exe.starts_with("/Applications/") {
        "/Applications".into()
    } else if !home.is_empty() && exe.starts_with(&format!("{home}/Applications/")) {
        "~/Applications".into()
    } else if exe.contains("/target/debug/") || exe.contains("/target/release/") {
        "dev build (cargo)".into()
    } else if exe.contains(".app/") {
        "app bundle elsewhere".into()
    } else {
        "other".into()
    }
}

fn readme(m: &Value, now: chrono::DateTime<chrono::Local>) -> String {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let daemon = &m["daemon"];
    let daemon_line = if daemon["running"] == true {
        let up = daemon["uptime_secs"].as_u64().map(|u| format!(", up {}", uptime(u))).unwrap_or_default();
        format!("running {}{up}", s(&daemon["version"]))
    } else {
        "not running".into()
    };
    let sys = &m["system"];
    let memory = sys["memory_gb"].as_u64().map(|g| format!(" · {g} GB")).unwrap_or_default();
    let mut out = format!(
        "# Taskboard debug export\n\nMade {} by {}.\n\n\
         | | |\n|---|---|\n\
         | Taskboard | {} ({}) |\n| Daemon | {daemon_line} |\n| macOS | {} ({}) |\n| Hardware | {} · {}{memory} |\n| Arch | {}{} |\n| Install | {} |\n\n",
        now.format("%b %-d, %Y at %-I:%M %p"),
        s(&m["made_by"]),
        s(&m["version"]),
        s(&m["channel"]),
        s(&sys["macos"]),
        s(&sys["macos_build"]),
        s(&sys["model"]),
        s(&sys["cpu"]),
        s(&sys["arch"]),
        if sys["rosetta"] == true { " (Rosetta)" } else { "" },
        s(&sys["install"]),
    );
    out.push_str(
        "## Contents\n\n\
         - `manifest.json`: the above, each table's row counts, and how many values were hidden.\n\
         - `board/schema.sql` and `board/<table>.json`: every table, newest rows first up to 20,000. Ids, \
         numbers, timestamps and status-like values are as they were; text columns holding JSON are \
         shown as JSON.\n\
         - `logs/`: the last 4 MB of each log in the data folder.\n\
         - `state/`: config, hooks, accounts (who, never tokens), the spool and statusline snapshots, and \
         `files.json`, the data folder's names and sizes.\n\n\
         ## Redaction\n\n\
         Every other string is a placeholder: `<kind:hash>`, where the kind is text, path, url, email, \
         user, host, secret, id or file. The same value has the same placeholder everywhere in this \
         export, so a project, branch or task title lines up across tables and logs; the hash is salted \
         per export, so it can't be guessed back. Attachments, task files, terminal output and Claude \
         transcripts aren't included.\n",
    );
    out
}

fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

// ------------------------------------------------------------------ zip

/// A zip of `entries`, each deflated, stamped `when`.
pub fn zip(entries: &[(String, Vec<u8>)], when: chrono::NaiveDateTime) -> Result<Vec<u8>, String> {
    use chrono::{Datelike, Timelike};
    let too_big = || "the export is too big for a zip".to_string();
    let time = ((when.hour() << 11) | (when.minute() << 5) | (when.second() / 2)) as u16;
    let date = (((when.year().clamp(1980, 2107) - 1980) as u32) << 9 | when.month() << 5 | when.day()) as u16;
    let mut out: Vec<u8> = vec![];
    let mut central: Vec<u8> = vec![];
    for (name, data) in entries {
        let mut enc = flate2::write::DeflateEncoder::new(vec![], flate2::Compression::default());
        enc.write_all(data).map_err(|e| e.to_string())?;
        let packed = enc.finish().map_err(|e| e.to_string())?;
        let crc = crc32fast::hash(data);
        let offset = u32::try_from(out.len()).map_err(|_| too_big())?;
        let (size, packed_size) = (u32::try_from(data.len()).map_err(|_| too_big())?, u32::try_from(packed.len()).map_err(|_| too_big())?);
        let name_len = name.len() as u16;
        let common = |b: &mut Vec<u8>| {
            for v in [20u16, 0x0800, 8, time, date] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for v in [crc, packed_size, size] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&name_len.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        common(&mut central);
        for v in [0u16, 0, 0] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let count = u16::try_from(entries.len()).map_err(|_| too_big())?;
    let cd_offset = u32::try_from(out.len()).map_err(|_| too_big())?;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    for v in [0u16, 0, count, count] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The zip's files, read back through their local headers.
    fn unzip(bytes: &[u8]) -> Vec<(String, String)> {
        let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]) as usize;
        let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let mut out = vec![];
        let mut i = 0;
        while u32_at(i) == 0x0403_4b50 {
            let (packed, name_len, extra) = (u32_at(i + 18), u16_at(i + 26), u16_at(i + 28));
            let name = String::from_utf8(bytes[i + 30..i + 30 + name_len].to_vec()).unwrap();
            let start = i + 30 + name_len + extra;
            let mut data = String::new();
            flate2::read::DeflateDecoder::new(&bytes[start..start + packed]).read_to_string(&mut data).unwrap();
            assert_eq!(crc32fast::hash(data.as_bytes()) as usize, u32_at(i + 14), "{name}'s checksum");
            out.push((name, data));
            i = start + packed;
        }
        assert_eq!(u32_at(i), 0x0201_4b50, "the central directory follows the files");
        out
    }

    const TOKEN: &str = "ghp_Zq8ExampleTokenValue0000000000000000";
    const ENV_SECRET: &str = "env-secret-plum-velvet-42";
    const APP_TOKEN: &str = "apptokenvalue-never-exported-0123456789";

    /// Strings the seeded board, logs and state hold that no export may contain.
    const SENSITIVE: &[&str] = &[
        "Rewrite the zebra-quartz billing flow",
        "moonlight-ledger",
        "/Users/alice",
        "alice@example.com",
        "zebra-quartz",
        "acme-corp",
        "ZQ-4242",
        "github.com/acme-corp",
        "feat/zq-secret-branch",
        "zq-secret-branch",
        "Nightjar",
        "nightjar",
        "alice-mbp",
        "Alice Example",
        TOKEN,
        ENV_SECRET,
        APP_TOKEN,
        "secret-screenshot",
        "PNGDATA-evidence-bytes",
        "terminal said: rm -rf build",
        "alice",
    ];

    fn seed(data: &Path) -> Config {
        let mut cfg = Config::for_tests(data);
        cfg.owner = "Alice Example".into();
        cfg.owner_emails = vec!["alice@example.com".into()];
        drop(crate::db::Db::open(&cfg.db_path()).unwrap());
        let conn = Connection::open(cfg.db_path()).unwrap();
        conn.execute_batch(&format!(
            r#"
            INSERT INTO goals(id, name, outcome, project, repo_path, epic_key, epic_status, created_at)
              VALUES (3, 'Project Nightjar launch', 'nightjar ships for acme-corp', 'zebra-quartz', '/Users/alice/Projects/zebra-quartz', 'ZQ-4000', 'In Progress', '2026-10-13T08:00:00-07:00');
            INSERT INTO tasks(id, title, detail, project, repo_path, status, priority, goal_id, jira_key, jira_status,
                              pr_host, pr_repo, pr_num, pr_url, pr_state, context, session_name, created_at, locks)
              VALUES (7, 'Rewrite the zebra-quartz billing flow',
                      'See /Users/alice/Projects/zebra-quartz/src/main.rs and ask alice@example.com',
                      'zebra-quartz', '/Users/alice/Projects/zebra-quartz', 'working', 'high', 3, 'ZQ-4242', 'In Review',
                      'github', 'acme-corp/zebra-quartz', 77, 'https://github.com/acme-corp/zebra-quartz/pull/77', 'open',
                      '{{"branch":"feat/zq-secret-branch","prompt":"Please fix the moonlight-ledger","waves":[1,2]}}',
                      'zebra-quartz T7', '2026-10-13T08:01:00-07:00', '["src/billing.rs"]');
            INSERT INTO events(task_id, at, who, kind, text, data)
              VALUES (7, '2026-10-13T08:02:00-07:00', 'alice', 'note', 'pushed with {TOKEN}', '{{"output":"terminal said: rm -rf build"}}');
            INSERT INTO events(task_id, at, who, kind, text) VALUES (7, '2026-10-13T08:03:00-07:00', 'agent', 'status', 'T7, T8');
            "#
        ))
        .unwrap();
        std::fs::write(
            cfg.log_path(),
            format!(
                "2026-10-13 08:01:00 started T7 Rewrite the zebra-quartz billing flow in /Users/alice/Projects/zebra-quartz\n\
                 2026-10-13 08:02:00 pushed feat/zq-secret-branch to https://github.com/acme-corp/zebra-quartz/pull/77 (acme-corp/zebra-quartz#77)\n\
                 2026-10-13 08:03:00 jira ZQ-4242 for alice@example.com on alice-mbp.local: token {TOKEN}\n\
                 2026-10-13 08:04:00 env {ENV_SECRET} via git@github.com:acme-corp/zebra-quartz.git, owner Alice Example (alice)\n\
                 2026-10-13 08:05:00 GET http://127.0.0.1:8787/tasks/api/state 200 for project \"zebra-quartz\"\n"
            ),
        )
        .unwrap();
        std::fs::write(crate::hooks::log_path(&cfg), "hook ran `make deploy-nightjar` in ~/Projects/zebra-quartz\n").unwrap();
        std::fs::write(&cfg.config_path, "owner = \"Alice Example\"\nowner_emails = [\"alice@example.com\"]\n[pr]\nwatch = true\n[pr.projects.nightjar]\napprovals = 1\n").unwrap();
        std::fs::write(cfg.data.join("accounts.json"), r#"{"github": {"login": "alice-mbp", "name": "Alice Example", "scopes": ["repo"]}}"#).unwrap();
        std::fs::create_dir_all(cfg.data.join("evidence")).unwrap();
        std::fs::write(cfg.data.join("evidence/secret-screenshot-zebra.png"), "PNGDATA-evidence-bytes").unwrap();
        std::fs::create_dir_all(cfg.data.join("tasks")).unwrap();
        std::fs::write(cfg.data.join("tasks/T7.md"), "Rewrite the zebra-quartz billing flow").unwrap();
        std::fs::create_dir_all(&cfg.statusline_dir).unwrap();
        std::fs::write(cfg.statusline_dir.join("s1.json"), r#"{"cwd": "/Users/alice/Projects/zebra-quartz", "model": {"id": "claude"}}"#).unwrap();
        std::fs::write(cfg.data.join(crate::apptoken::FILE), APP_TOKEN).unwrap();
        cfg
    }

    fn sources() -> Sources {
        Sources {
            home: "/Users/alice".into(),
            user: "alice".into(),
            hosts: vec!["alice-mbp.local".into()],
            names: vec!["Alice Example".into()],
            emails: vec![],
            secrets: vec![ENV_SECRET.into()],
        }
    }

    fn export(cfg: &Config) -> Vec<(String, String)> {
        let now = chrono::Local::now();
        let entries = bundle(cfg, "test", [1, 2], &sources(), json!({"running": true, "version": "0.1.0", "uptime_secs": 4000}), now).unwrap();
        unzip(&zip(&entries, now.naive_local()).unwrap())
    }

    fn file<'a>(files: &'a [(String, String)], name: &str) -> &'a str {
        &files.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name} is in the export")).1
    }

    #[test]
    fn a_seeded_board_exports_none_of_its_sensitive_strings() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = seed(dir.path());
        let files = export(&cfg);
        for (name, text) in &files {
            for s in SENSITIVE {
                let (name_l, text_l, s_l) = (name.to_lowercase(), text.to_lowercase(), s.to_lowercase());
                assert!(!name_l.contains(&s_l), "{name} names {s}");
                assert!(!text_l.contains(&s_l), "{name} holds {s}:\n{text}");
            }
        }
        for name in ["README.md", "manifest.json", "board/tasks.json", "board/goals.json", "board/schema.sql", "logs/server.log", "logs/hooks.log", "state/config.json", "state/accounts.json", "state/statusline/1.json", "state/files.json"] {
            file(&files, name);
        }
    }

    #[test]
    fn ids_states_and_times_survive_and_placeholders_line_up() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = seed(dir.path());
        let files = export(&cfg);
        let tasks: Value = serde_json::from_str(file(&files, "board/tasks.json")).unwrap();
        let goals: Value = serde_json::from_str(file(&files, "board/goals.json")).unwrap();
        let task = &tasks["rows"][0];
        let goal = &goals["rows"][0];
        assert_eq!(task["id"], 7);
        assert_eq!(task["goal_id"], 3);
        assert_eq!(task["pr_num"], 77);
        assert_eq!(task["status"], "working");
        assert_eq!(task["pr_state"], "open");
        assert_eq!(task["jira_status"], "In Review");
        assert_eq!(task["created_at"], "2026-10-13T08:01:00-07:00");
        assert_eq!(task["context"]["waves"], json!([1, 2]), "JSON columns stay JSON");
        let project = task["project"].as_str().unwrap();
        assert!(project.starts_with("<text:"), "{project}");
        assert_eq!(goal["project"], project, "the same project has the same placeholder in every table");
        let log = file(&files, "logs/server.log");
        assert!(log.contains(project), "and in the logs:\n{log}");
        assert!(log.contains(task["title"].as_str().unwrap()), "{log}");
        assert!(log.contains("http://127.0.0.1:8787/tasks/api/state"), "the board's own address stays:\n{log}");
        assert!(log.contains("2026-10-13 08:01:00 started T7"), "{log}");
        assert!(!log.contains("<<"), "placeholders aren't redacted again:\n{log}");
        assert!(log.contains("(<user:"), "the user is a user wherever it appears:\n{log}");
        let events: Value = serde_json::from_str(file(&files, "board/events.json")).unwrap();
        assert_eq!(events["rows"][1]["text"], "T7, T8");
        let manifest: Value = serde_json::from_str(file(&files, "manifest.json")).unwrap();
        assert_eq!(manifest["tables"]["tasks"]["total"], 1);
        assert_eq!(manifest["daemon"]["uptime_secs"], 4000);
        assert!(file(&files, "README.md").contains("running 0.1.0, up 1h 6m"));
    }

    #[test]
    fn a_new_salt_makes_new_placeholders() {
        let cfg = Config::for_tests(Path::new("/nowhere"));
        let mut a = Redactor::new([1, 2], &Sources::default(), &cfg);
        let mut b = Redactor::new([3, 4], &Sources::default(), &cfg);
        assert_eq!(a.hide("zebra-quartz"), a.hide("zebra-quartz"));
        assert_ne!(a.hide("zebra-quartz"), b.hide("zebra-quartz"));
    }

    #[test]
    fn install_kind_hides_the_home_folder() {
        assert_eq!(install_kind("/Applications/Taskboard.app/Contents/MacOS/taskboard", "/Users/x"), "/Applications");
        assert_eq!(install_kind("/Users/x/Applications/Taskboard.app/Contents/MacOS/taskboard", "/Users/x"), "~/Applications");
        assert_eq!(install_kind("/Users/x/dev/taskboard/target/debug/tb", "/Users/x"), "dev build (cargo)");
    }
}
