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
}

impl Daemon {
    #[cfg(test)]
    pub fn at(base: &str) -> Daemon {
        Daemon { base: base.into(), agent: ureq::AgentBuilder::new().timeout_connect(Duration::from_millis(300)).build() }
    }

    pub fn new() -> Daemon {
        let cfg = Config::load().ok();
        let host = cfg.as_ref().map(|c| c.host.clone()).unwrap_or_else(|| "127.0.0.1".into());
        let port = cfg.as_ref().map(|c| c.port).unwrap_or(8792);
        let base = std::env::var("TASKBOARD_URL").ok().map(|u| u.trim_end_matches('/').trim_end_matches("/tasks").to_string());
        Daemon {
            base: base.unwrap_or_else(|| format!("http://{host}:{port}")),
            agent: ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(2)).timeout(Duration::from_secs(20)).build(),
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
        let req = self.agent.post(&format!("{}/tasks/api/{path}", self.base)).set("X-Task-Board", "1");
        self.answer(req.send_json(body))
    }
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
        api::dispatch(&self.board, "POST", path, &Query::new(), &body).map_err(|e| CallError { status: e.status, message: e.message })
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
}
