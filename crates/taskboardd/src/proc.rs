//! Running a command with a time limit.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub enum RunError {
    Spawn(std::io::Error),
    TimedOut,
}

pub fn run(program: &Path, args: &[String], cwd: Option<&Path>, timeout: f64) -> Result<Output, RunError> {
    run_with(program, args, cwd, timeout, &[], None)
}

/// `run` with extra environment variables and, optionally, bytes written to the command's stdin.
pub fn run_with(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    timeout: f64,
    env: &[(String, String)],
    input: Option<&[u8]>,
) -> Result<Output, RunError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.envs(env.iter().map(|(k, v)| (k, v)));
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    let mut child = cmd.spawn().map_err(RunError::Spawn)?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        let bytes = bytes.to_vec();
        // A command that never reads its stdin mustn't hold up the time limit.
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        });
    }
    let mut out = child.stdout.take().unwrap();
    let mut errp = child.stderr.take().unwrap();
    let (otx, orx) = std::sync::mpsc::channel();
    let (etx, erx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out.read_to_end(&mut s);
        let _ = otx.send(s);
    });
    std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = errp.read_to_end(&mut s);
        let _ = etx.send(s);
    });
    let end = Instant::now() + Duration::from_secs_f64(timeout.max(0.1));
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if Instant::now() >= end => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => break None,
        }
    };
    // A helper process the command left behind can hold its pipes open; don't wait on it for long.
    let grace = Duration::from_secs(2);
    let stdout = String::from_utf8_lossy(&orx.recv_timeout(grace).unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&erx.recv_timeout(grace).unwrap_or_default()).into_owned();
    Ok(Output { code: status.and_then(|s| s.code()), stdout, stderr })
}

/// Finds a command on PATH (or returns the path as given when it has a slash).
pub fn which(name: &str) -> Option<std::path::PathBuf> {
    if name.contains('/') {
        let p = crate::util::expand_home(name);
        return if p.is_file() { Some(p) } else { None };
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    for extra in ["~/.local/bin", "/opt/homebrew/bin", "/usr/local/bin"] {
        let p = crate::util::expand_home(extra).join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}
