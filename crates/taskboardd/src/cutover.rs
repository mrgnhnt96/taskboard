//! Moving over from another board (the Python board this one replaces): what's in the way. Another
//! program on the board's port, another `tb` first on the PATH, another task-board plugin in Claude
//! Code. The app checks these on launch and `taskboardd serve` when its port is taken, and each says
//! so in a sentence instead of failing quietly. The steps are in docs/CUTOVER.md.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

/// Who answers on the board's port.
#[derive(Debug, Clone, PartialEq)]
pub enum Listener {
    Nobody,
    /// taskboardd, with its version.
    Ours(String),
    /// Something else: what it said, in short.
    Other(String),
}

/// Asks `GET /` on host:port. taskboardd answers `{"board": "taskboardd", "version": …}`.
pub fn who_listens(host: &str, port: u16) -> Listener {
    let Some(addr) = (host, port).to_socket_addrs().ok().and_then(|mut a| a.next()) else { return Listener::Nobody };
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else { return Listener::Nobody };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    if s.write_all(format!("GET / HTTP/1.0\r\nHost: {host}:{port}\r\nAccept: application/json\r\n\r\n").as_bytes()).is_err() {
        return Listener::Other("it took the connection but didn't answer".into());
    }
    let mut buf = Vec::new();
    let _ = s.take(64 * 1024).read_to_end(&mut buf);
    classify(&String::from_utf8_lossy(&buf))
}

/// What an HTTP answer to `GET /` says about who sent it.
pub fn classify(resp: &str) -> Listener {
    let body = resp.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    if let Ok(v) = serde_json::from_str::<Value>(body.trim()) {
        if v["board"] == "taskboardd" {
            return Listener::Ours(v["version"].as_str().unwrap_or("").to_string());
        }
    }
    let status = resp.lines().next().unwrap_or("").trim();
    let server = resp.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("server")).map(|(_, v)| v.trim().to_string()));
    let lower = body.to_lowercase();
    let what = if lower.contains("task board") || lower.contains("taskboard") {
        "another task board (the old one?)".to_string()
    } else if let Some(s) = server {
        format!("a server that calls itself {s}")
    } else if status.is_empty() {
        "a program that doesn't speak HTTP".to_string()
    } else {
        "a program that isn't taskboardd".to_string()
    };
    Listener::Other(what)
}

/// Whether the file at `p` is this board's `tb`: the binary itself, a link to a Taskboard bundle's,
/// or the plugin's shim that runs it.
pub fn is_our_tb(p: &Path) -> bool {
    let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = real.to_string_lossy();
    if s.contains("Taskboard") && s.ends_with(".app/Contents/MacOS/tb") {
        return true;
    }
    let Ok(bytes) = std::fs::read(&real) else { return false };
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    if text.starts_with("#!") {
        return text.contains("Shim: run the installed Rust tb binary");
    }
    // A built `tb`: clap's about line is in it.
    bytes.windows(30).any(|w| w == b"Report to the local task board")
}

/// The first `tb` on `path_var` (a PATH value), when it isn't this board's.
pub fn foreign_tb(path_var: &str) -> Option<PathBuf> {
    let first = path_var.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("tb")).find(|p| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })?;
    (!is_our_tb(&first)).then_some(first)
}

/// Task-board plugins in Claude Code's `installed_plugins.json` other than this board's
/// (`task-board@taskboard` with the shim in its `bin/`).
pub fn other_plugins(claude_dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(claude_dir.join("plugins/installed_plugins.json")) else { return vec![] };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return vec![] };
    let Some(plugins) = v["plugins"].as_object() else { return vec![] };
    let mut out = vec![];
    for (name, installs) in plugins {
        let base = name.split('@').next().unwrap_or("").to_lowercase().replace(['-', '_'], "");
        if base != "taskboard" {
            continue;
        }
        let ours = installs.as_array().is_some_and(|a| {
            a.iter().filter_map(|i| i["installPath"].as_str()).any(|p| {
                let tb = Path::new(p).join("bin/tb");
                !tb.exists() || is_our_tb(&tb)
            })
        });
        if name != "task-board@taskboard" || !ours {
            out.push(name.clone());
        }
    }
    out
}

/// The sentences for what's in the way, if anything (this board's own daemon on the port is fine).
pub fn problems(host: &str, port: u16, path_var: &str, claude_dir: &Path) -> Vec<String> {
    let mut out = vec![];
    if let Listener::Other(what) = who_listens(host, port) {
        out.push(format!(
            "Something else is listening on {host}:{port}: {what}. Stop it (for the old board: launchctl bootout its LaunchAgent), or give this board another port in config.toml."
        ));
    }
    if let Some(tb) = foreign_tb(path_var) {
        out.push(format!("The first tb on the PATH is {}, which isn't this board's, so agents would report to another board. Remove it or put ~/.local/bin first.", tb.display()));
    }
    for p in other_plugins(claude_dir) {
        out.push(format!("Claude Code has another task-board plugin installed ({p}). Uninstall it with claude plugin uninstall {p}, so its hooks don't report to the old board."));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_this_board_from_others() {
        let ours = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"board\":\"taskboardd\",\"version\":\"0.1.0\"}";
        assert_eq!(classify(ours), Listener::Ours("0.1.0".into()));
        let old = "HTTP/1.0 200 OK\r\nServer: SimpleHTTP/0.6 Python/3.12\r\n\r\n<html><title>Task board</title></html>";
        assert_eq!(classify(old), Listener::Other("another task board (the old one?)".into()));
        let other = "HTTP/1.0 404 Not Found\r\nServer: nginx\r\n\r\nnope";
        assert_eq!(classify(other), Listener::Other("a server that calls itself nginx".into()));
        assert!(matches!(classify(""), Listener::Other(_)));
    }

    #[test]
    fn asks_whoever_listens() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\nServer: Python/3.12\r\n\r\n<h1>Task board</h1>");
            }
        });
        assert_eq!(who_listens("127.0.0.1", port), Listener::Other("another task board (the old one?)".into()));
    }

    #[test]
    fn finds_a_foreign_tb_first_on_the_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("tb-path-test-{}", std::process::id()));
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let write = |p: &Path, text: &str| {
            std::fs::write(p, text).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        write(&a.join("tb"), "#!/usr/bin/env python3\n# the old board's tb\n");
        write(&b.join("tb"), "#!/bin/sh\n# Shim: run the installed Rust tb binary.\n");
        let path = format!("{}:{}", a.display(), b.display());
        assert_eq!(foreign_tb(&path), Some(a.join("tb")));
        let path = format!("{}:{}", b.display(), a.display());
        assert_eq!(foreign_tb(&path), None, "ours comes first");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_other_task_board_plugins() {
        let dir = std::env::temp_dir().join(format!("tb-plugins-test-{}", std::process::id()));
        let ours = dir.join("cache/ours");
        std::fs::create_dir_all(ours.join("bin")).unwrap();
        std::fs::write(ours.join("bin/tb"), "#!/bin/sh\n# Shim: run the installed Rust tb binary.\n").unwrap();
        std::fs::create_dir_all(dir.join("plugins")).unwrap();
        let v = serde_json::json!({"plugins": {
            "task-board@taskboard": [{"installPath": ours}],
            "task-board@old-board": [{"installPath": "/nowhere"}],
            "swift-lsp@official": [{"installPath": "/x"}]}});
        std::fs::write(dir.join("plugins/installed_plugins.json"), v.to_string()).unwrap();
        assert_eq!(other_plugins(&dir), vec!["task-board@old-board".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
