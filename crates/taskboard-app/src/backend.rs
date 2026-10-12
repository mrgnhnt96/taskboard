//! The app's view of taskboardd.
//!
//! `Backend` is a thin seam over the board's JSON API (`crates/taskboardd/web/CONTRACT.md`
//! documents every shape). The real implementation (`Daemon`) speaks HTTP to the daemon on
//! `127.0.0.1:<port>/tasks/api`; the fake one (`TASKBOARD_BACKEND=fake`) runs the board itself
//! in-process on a seeded temp folder, with no runner, so the UI can be developed and
//! screenshotted without a daemon or Midna.
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use taskboardd::api::{self, Query};
use taskboardd::app::App as Board;
use taskboardd::config::Config;

/// A failed call: the board's `{"error": …}` sentence, or why it couldn't be reached.
#[derive(Clone, Debug)]
pub struct CallError {
    /// The HTTP status (0: unreachable). The UI shows only `message`, as the web board did;
    /// tests check the status.
    #[cfg_attr(not(test), allow(dead_code))]
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type CallResult = Result<Value, CallError>;

pub trait Backend: Send + Sync + 'static {
    fn label(&self) -> &'static str;
    /// `GET /tasks/api/<path>?<query>`. Blocking; call from a background task.
    fn get(&self, path: &str, query: &[(&str, String)]) -> CallResult;
    /// `POST /tasks/api/<path>` with a JSON body. Blocking; call from a background task.
    fn post(&self, path: &str, body: Value) -> CallResult;
}

/// Pick the backend: `TASKBOARD_BACKEND=fake` for the in-process sample board, otherwise the daemon.
pub fn from_env() -> Arc<dyn Backend> {
    match std::env::var("TASKBOARD_BACKEND").as_deref() {
        Ok("fake") => Arc::new(Fake::new()),
        _ => Arc::new(Daemon::new()),
    }
}

// ------------------------------------------------------------------ daemon

pub struct Daemon {
    base: String,
    agent: ureq::Agent,
    /// Where the running daemon keeps this launch's app token (`taskboardd::apptoken`).
    token: Option<std::path::PathBuf>,
    /// The daemon's app socket (`taskboardd::apporigin`), where the app's posts go: a team-signed
    /// daemon takes them as the app's from the signed app only. None with `TASKBOARD_URL`.
    socket: Option<std::path::PathBuf>,
}

impl Daemon {
    #[cfg(test)]
    pub fn at(base: &str) -> Daemon {
        Daemon { base: base.into(), agent: ureq::AgentBuilder::new().timeout_connect(Duration::from_millis(300)).build(), token: None, socket: None }
    }

    pub fn new() -> Daemon {
        let cfg = Config::load().ok();
        let host = cfg.as_ref().map(|c| c.host.clone()).unwrap_or_else(|| "127.0.0.1".into());
        let port = cfg.as_ref().map(|c| c.port).unwrap_or(8792);
        let base = std::env::var("TASKBOARD_URL").ok().map(|u| u.trim_end_matches('/').trim_end_matches("/tasks").to_string());
        Daemon {
            socket: if base.is_some() { None } else { cfg.as_ref().map(taskboardd::apporigin::socket_path) },
            base: base.unwrap_or_else(|| format!("http://{host}:{port}")),
            agent: ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(2)).timeout(Duration::from_secs(20)).build(),
            token: cfg.as_ref().map(taskboardd::apptoken::path),
        }
    }

    fn answer(&self, r: Result<ureq::Response, ureq::Error>) -> CallResult {
        match r {
            // As the web board's `api()`: an OK answer that isn't JSON is just empty.
            Ok(resp) => Ok(resp.into_json::<Value>().unwrap_or(Value::Null)),
            Err(ureq::Error::Status(code, resp)) => {
                let msg = resp.into_json::<Value>().ok().and_then(|v| v["error"].as_str().map(str::to_string));
                Err(CallError { status: code, message: msg.unwrap_or_else(|| format!("The board answered {code}.")) })
            }
            Err(_) => Err(CallError { status: 0, message: "Can’t reach the task board server.".into() }),
        }
    }
}

impl Backend for Daemon {
    fn label(&self) -> &'static str {
        "daemon"
    }


    fn get(&self, path: &str, query: &[(&str, String)]) -> CallResult {
        let mut req = self.agent.get(&format!("{}/tasks/api/{path}", self.base)).set("Accept", "application/json");
        for (k, v) in query {
            req = req.query(k, v);
        }
        self.answer(req.call())
    }

    fn post(&self, path: &str, body: Value) -> CallResult {
        // `X-Task-Board-From: app`: the owner's own click, which the board takes for what only the
        // owner may do (Start, a wave's review stop) when it comes from this signed app on the app
        // socket, or, from a daemon with no team signature, with this launch's app token. The token is
        // read each time, so a restarted daemon's new one is picked up.
        let token = self.token.as_deref().and_then(taskboardd::apptoken::read);
        if let Some(sock) = self.socket.as_deref().filter(|s| s.exists()) {
            let mut headers = vec![("X-Task-Board", "1"), ("X-Task-Board-From", "app")];
            if let Some(t) = token.as_deref() {
                headers.push(("X-Task-Board-Token", t));
            }
            let bytes = serde_json::to_vec(&body).unwrap_or_default();
            // A stale socket (nothing listens) falls back to the port; a post that got there doesn't
            // go twice.
            match taskboardd::apporigin::post(sock, path, &headers, &bytes, Duration::from_secs(20)) {
                Ok((status, answer)) => return socket_answer(status, &answer),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => {}
                Err(_) => return Err(CallError { status: 0, message: "Can’t reach the task board server.".into() }),
            }
        }
        let mut req = self.agent.post(&format!("{}/tasks/api/{path}", self.base)).set("X-Task-Board", "1").set("X-Task-Board-From", "app");
        if let Some(token) = token {
            req = req.set("X-Task-Board-Token", &token);
        }
        self.answer(req.send_json(body))
    }
}

/// An answer from the app socket, read as `Daemon::answer` reads one from the port.
fn socket_answer(status: u16, body: &[u8]) -> CallResult {
    let v = serde_json::from_slice::<Value>(body).ok();
    if (200..300).contains(&status) {
        return Ok(v.unwrap_or(Value::Null));
    }
    let msg = v.and_then(|v| v["error"].as_str().map(str::to_string));
    Err(CallError { status, message: msg.unwrap_or_else(|| format!("The board answered {status}.")) })
}

// ------------------------------------------------------------------ fake

/// The real board code on a throwaway seeded folder: every API call goes straight to
/// `api::dispatch`, so the fake can never drift from the daemon's shapes.
pub struct Fake {
    board: Arc<Board>,
    dir: std::path::PathBuf,
}

impl Fake {
    pub fn new() -> Fake {
        // One folder per board: tests open many in one process, in parallel.
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("taskboard-fake-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fake board folder");
        let mut cfg = Config::for_tests(&dir);
        cfg.owner = "Alex".into();
        let board = Board::new(cfg, false).expect("fake board");
        if let Err(e) = taskboardd::seed::seed(&board) {
            eprintln!("taskboard-app: seeding the fake board failed: {}", e.message);
        }
        seed_sessions(&board);
        Fake { board, dir }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Backend for Fake {
    fn label(&self) -> &'static str {
        "fake"
    }


    fn get(&self, path: &str, query: &[(&str, String)]) -> CallResult {
        let q: Query = query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        api::dispatch(&self.board, "GET", path, &q, &json!({})).map_err(|e| CallError { status: e.status, message: e.message })
    }

    fn post(&self, path: &str, body: Value) -> CallResult {
        let q: Query = [(api::FROM.to_string(), "app".to_string())].into_iter().collect();
        api::dispatch(&self.board, "POST", path, &q, &body).map_err(|e| CallError { status: e.status, message: e.message })
    }
}

/// A few live terminals for the Sessions page and the board's terminal strip.
fn seed_sessions(board: &Board) {
    use taskboardd::util::now_iso;
    let now = now_iso();
    let rows = [
        ("fake-s1", "T2 Sign in with a passkey", "webapp", "working", Some(2i64)),
        ("fake-s2", "T3 Settings page: manage passkeys", "webapp", "needs", Some(3)),
        ("fake-s3", "api shell", "api", "idle", None),
    ];
    for (id, name, project, status, task) in rows {
        let _ = board.db.insert(
            "sessions",
            taskboardd::fields!["id" => id, "name" => name, "project" => project, "project_path" => format!("/tmp/{project}"),
                                "status" => status, "agent" => "claude", "last_activity" => now.clone(), "seen_at" => now.clone(),
                                "branch" => "feature/passkeys"],
        );
        if let Some(t) = task {
            let _ = board.db.x("UPDATE tasks SET session_id = ?, session_name = ? WHERE id = ?", taskboardd::p![id, name, t]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same sentences the web board's `api()` showed.
    #[::core::prelude::v1::test]
    fn errors_read_like_the_web() {
        // Nothing listens on port 9 (discard) on a dev machine.
        let d = Daemon::at("http://127.0.0.1:9");
        let e = d.get("state", &[]).unwrap_err();
        assert_eq!(e.message, "Can’t reach the task board server.");
        assert_eq!(e.status, 0);
    }

    /// Posts go to the app socket when there's one, saying they're the app's and carrying the token
    /// when the daemon wrote one; the port is the fallback.
    #[::core::prelude::v1::test]
    fn posts_go_to_the_app_socket() {
        use std::io::{Read, Write};
        let dir = std::env::temp_dir().join(format!("tb-app-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("app.sock");
        let token = dir.join("app-token");
        std::fs::write(&token, "abc123\n").unwrap();
        let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let mut seen = vec![];
            for answer in ["HTTP/1.1 200 OK\r\ncontent-length: 11\r\n\r\n{\"ok\":true}", "HTTP/1.1 403 Forbidden\r\ncontent-length: 16\r\n\r\n{\"error\":\"Nope\"}"] {
                let (mut s, _) = l.accept().unwrap();
                let mut buf = vec![0u8; 4096];
                let mut req = vec![];
                while !String::from_utf8_lossy(&req).contains("{\"mode\"") {
                    let n = s.read(&mut buf).unwrap();
                    req.extend_from_slice(&buf[..n]);
                }
                seen.push(String::from_utf8_lossy(&req).to_string());
                s.write_all(answer.as_bytes()).unwrap();
            }
            seen
        });
        let mut d = Daemon::at("http://127.0.0.1:9");
        d.socket = Some(sock);
        d.token = Some(token);
        assert_eq!(d.post("tasks/T1/start", json!({"mode": "queue"})).unwrap(), json!({"ok": true}));
        let e = d.post("tasks/T1/start", json!({"mode": "queue"})).unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (403, "Nope"));
        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("POST /tasks/api/tasks/T1/start HTTP/1.1\r\n"), "{}", seen[0]);
        for h in ["X-Task-Board: 1\r\n", "X-Task-Board-From: app\r\n", "X-Task-Board-Token: abc123\r\n"] {
            assert!(seen[0].contains(h), "{h} in {}", seen[0]);
        }
        // No socket: the port, which isn't there.
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(d.post("tasks/T1/start", json!({})).unwrap_err().status, 0);
    }
}
