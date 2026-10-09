//! What the hooks and `tb` share: where the board is, git facts, the spool, and requests.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use taskboardd::config::Config;
use taskboardd::util::expand_home;

pub fn config() -> Config {
    Config::load().unwrap_or_else(|e| {
        eprintln!("tb: {e}; using the defaults");
        Config::from_file(Default::default(), taskboardd::config::default_config_path())
    })
}

pub fn base_url(cfg: &Config) -> String {
    cfg.url().trim_end_matches('/').to_string()
}

pub fn api_url(cfg: &Config, path: &str) -> String {
    format!("{}/tasks/api{path}", base_url(cfg))
}

pub fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[derive(Debug)]
pub enum CallError {
    /// The board answered with an error status (its sentence): it saw the report and refused it.
    Refused(String),
    /// The board couldn't be reached (or answered with something that isn't JSON).
    Unreachable(String),
}

pub fn request(cfg: &Config, method: &str, path: &str, body: Option<&Value>, timeout: f64) -> Result<Value, CallError> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs_f64(timeout.max(0.05))).try_proxy_from_env(false).build();
    let req = agent.request(method, &api_url(cfg, path)).set("X-Task-Board", "1").set("Accept", "application/json");
    let resp = match body {
        Some(b) => req.send_json(b.clone()),
        None => req.call(),
    };
    match resp {
        Ok(r) => {
            let text = r.into_string().map_err(|e| CallError::Unreachable(e.to_string()))?;
            if text.trim().is_empty() {
                return Ok(json!({}));
            }
            serde_json::from_str(&text).map_err(|_| CallError::Unreachable("the reply wasn't JSON".into()))
        }
        Err(ureq::Error::Status(code, r)) => {
            let msg = r
                .into_string()
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v["error"].as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| format!("HTTP {code}"));
            Err(CallError::Refused(msg))
        }
        Err(e) => Err(CallError::Unreachable(e.to_string())),
    }
}

pub fn spool_write(cfg: &Config, body: &Value) -> std::io::Result<PathBuf> {
    use rand::Rng;
    let dir = cfg.spool_dir();
    std::fs::create_dir_all(&dir)?;
    let ms = chrono::Utc::now().timestamp_millis();
    let name = format!("{ms:013}-{}-{:06x}.json", std::process::id(), rand::thread_rng().gen::<u32>() & 0xff_ffff);
    let tmp = dir.join(format!(".{name}.tmp"));
    let fin = dir.join(&name);
    {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(body.to_string().as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &fin)?;
    Ok(fin)
}

fn spawn_git(cwd: &str, args: &[&str]) -> Option<std::process::Child> {
    Command::new("git")
        .args(["-C", cwd, "--no-optional-locks"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

fn finish(child: Option<std::process::Child>, deadline: Instant) -> Option<String> {
    let mut c = child?;
    let mut pipe = c.stdout.take()?;
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut out = String::new();
        pipe.read_to_string(&mut out).ok().map(|_| out)
    });
    loop {
        match c.try_wait() {
            Ok(Some(st)) => {
                if !st.success() {
                    return None;
                }
                return reader.join().ok().flatten();
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = c.kill();
                let _ = c.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => return None,
        }
    }
}

/// Branch, last commit and uncommitted count, each git call capped so hooks stay fast.
pub fn git_info(cwd: &str, timeout: f64) -> Value {
    if cwd.is_empty() || !std::path::Path::new(cwd).is_dir() {
        return Value::Null;
    }
    let procs = [
        spawn_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]),
        spawn_git(cwd, &["log", "-1", "--format=%h%x09%s"]),
        spawn_git(cwd, &["status", "--porcelain", "--untracked-files=no"]),
        spawn_git(cwd, &["status", "--porcelain", "--branch"]),
    ];
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    let mut it = procs.into_iter();
    let branch = finish(it.next().flatten(), deadline);
    let log = finish(it.next().flatten(), deadline);
    let tracked = finish(it.next().flatten(), deadline);
    let status = finish(it.next().flatten(), deadline);
    let mut lines: Vec<&str> = status.as_deref().unwrap_or("").lines().collect();
    let header = lines.first().filter(|l| l.starts_with("## ")).map(|s| s.to_string());
    if header.is_some() {
        lines.remove(0);
    }
    let branch = match branch {
        Some(b) => b.trim().to_string(),
        None => match &header {
            None => return Value::Null,
            Some(h) => {
                let mut b = h[3..].split("...").next().unwrap_or("").to_string();
                for p in ["No commits yet on ", "Initial commit on "] {
                    if let Some(rest) = b.strip_prefix(p) {
                        b = rest.to_string();
                    }
                }
                b.trim().to_string()
            }
        },
    };
    let (sha, subject) = match &log {
        Some(l) => {
            let l = l.trim();
            let (a, b) = l.split_once('\t').unwrap_or((l, ""));
            (a.to_string(), b.to_string())
        }
        None => (String::new(), String::new()),
    };
    let uncommitted = if status.is_some() {
        json!(lines.iter().filter(|l| !l.trim().is_empty()).count())
    } else if let Some(t) = &tracked {
        json!(t.lines().filter(|l| !l.trim().is_empty()).count())
    } else {
        Value::Null
    };
    json!({"branch": branch, "commit": subject, "sha": sha, "uncommitted": uncommitted})
}

const TREE_FILES_MAX: usize = 2000;
const REFLOG_KEPT: &str = "30";
/// Past this many bytes in one file, or in all of a stamp's files together, a file is stamped by
/// mtime and size instead of its content, so the hook stays fast.
const HASH_FILE_MAX: u64 = 8 * 1024 * 1024;
const HASH_TOTAL_MAX: u64 = 64 * 1024 * 1024;

/// The checkout's root, full HEAD, its newest reflog entries and a stamp (a hash of the content) per
/// changed or untracked file, so the board can tell whether a turn changed code however it did it
/// (an edit tool, a Bash script, a subagent or a commit) and how HEAD moved.
pub fn tree_stamp(cwd: &str, timeout: f64) -> Value {
    if cwd.is_empty() || !std::path::Path::new(cwd).is_dir() {
        return Value::Null;
    }
    let rev = spawn_git(cwd, &["rev-parse", "--show-toplevel", "HEAD"]);
    let status = spawn_git(cwd, &["status", "--porcelain", "-z", "--untracked-files=all"]);
    let reflog = spawn_git(cwd, &["reflog", "-n", REFLOG_KEPT, "--date=unix", "--format=%H%x09%gd%x09%gs", "HEAD"]);
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    let (Some(rev), Some(status)) = (finish(rev, deadline), finish(status, deadline)) else { return Value::Null };
    let reflog = finish(reflog, deadline).unwrap_or_default();
    let mut rev = rev.lines();
    let (Some(root), head) = (rev.next().map(str::trim).filter(|r| !r.is_empty()), rev.next().unwrap_or("").trim()) else {
        return Value::Null;
    };
    let mut files = serde_json::Map::new();
    let mut budget = HASH_TOTAL_MAX;
    let mut entries = status.split('\0');
    while let Some(entry) = entries.next() {
        if entry.len() < 4 || files.len() >= TREE_FILES_MAX {
            continue;
        }
        let (code, path) = entry.split_at(3);
        if code.contains('R') || code.contains('C') {
            entries.next();
        }
        let full = format!("{root}/{path}");
        files.insert(full.clone(), json!(file_stamp(&full, &mut budget)));
    }
    json!({"root": root, "head": head, "files": files, "reflog": reflog_entries(&reflog)})
}

/// `git reflog --date=unix --format=%H%x09%gd%x09%gs` lines, newest first, as `{sha, at, how}`.
fn reflog_entries(out: &str) -> Vec<Value> {
    out.lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            let (sha, sel, how) = (parts.next()?, parts.next()?, parts.next().unwrap_or(""));
            let at = sel.rsplit_once('{').and_then(|(_, t)| t.trim_end_matches('}').parse::<i64>().ok()).unwrap_or(0);
            Some(json!({"sha": sha, "at": at, "how": how}))
        })
        .collect()
}

fn file_stamp(path: &str, budget: &mut u64) -> String {
    let Ok(m) = std::fs::metadata(path) else { return "gone".into() };
    if m.is_file() && m.len() <= HASH_FILE_MAX && m.len() <= *budget {
        if let Ok(bytes) = std::fs::read(path) {
            *budget -= m.len();
            // FNV-1a over the content: a touch, or a save of the same bytes, reads as no change.
            let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3));
            return format!("h{hash:016x}:{}", m.len());
        }
    }
    let nanos = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{nanos}:{}", m.len())
}

pub fn base_body(event: &str, session: &str, claude_session: &str, cwd: &str, git: Value) -> Value {
    json!({"event": event, "session": session, "claude_session": claude_session, "cwd": cwd, "git": git, "at": now_iso()})
}

pub fn midna_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TASKBOARD_MIDNA") {
        if !p.is_empty() {
            return Some(expand_home(&p));
        }
    }
    let d = expand_home("~/.local/bin/midna");
    if d.is_file() {
        return Some(d);
    }
    taskboardd::proc::which("midna")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) {
        assert!(Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success(), "git {args:?}");
    }

    #[test]
    fn stamps_a_checkouts_head_and_changed_files() {
        let dir = std::env::temp_dir().join(format!("tb-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("src/a.rs"), "a").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "a"]);
        let root = std::fs::canonicalize(&dir).unwrap().to_string_lossy().to_string();
        let clean = tree_stamp(&dir.to_string_lossy(), 5.0);
        assert_eq!(clean["root"], json!(root));
        assert_eq!(clean["head"].as_str().map(str::len), Some(40));
        assert_eq!(clean["files"], json!({}));

        std::fs::write(dir.join("src/a.rs"), "changed").unwrap();
        std::fs::write(dir.join("src/new.rs"), "new").unwrap();
        let dirty = tree_stamp(&dir.to_string_lossy(), 5.0);
        let files: Vec<&String> = dirty["files"].as_object().unwrap().keys().collect();
        assert_eq!(files, [&format!("{root}/src/a.rs"), &format!("{root}/src/new.rs")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stamp_follows_content_and_reads_the_reflog() {
        let dir = std::env::temp_dir().join(format!("tb-tree-content-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a.rs"), "a").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "first one"]);
        std::fs::write(dir.join("a.rs"), "dirty").unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let before = tree_stamp(&cwd, 5.0);
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("a.rs"), "dirty").unwrap();
        let touched = tree_stamp(&cwd, 5.0);
        assert_eq!(before["files"], touched["files"], "the same bytes saved again are the same stamp");

        git(&dir, &["checkout", "-qb", "other"]);
        let reflog = tree_stamp(&cwd, 5.0)["reflog"].clone();
        assert!(reflog[0]["how"].as_str().unwrap().starts_with("checkout: moving from "), "{reflog}");
        assert_eq!(reflog[1]["how"], json!("commit (initial): first one"));
        assert!(reflog[1]["at"].as_i64().unwrap() > 1_700_000_000, "{reflog}");
        assert_eq!(reflog[1]["sha"], before["head"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
