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
