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
    /// Why the board was last out of reach (None: it answered). Logged when it changes, not on
    /// every poll.
    down: std::sync::Mutex<Option<String>>,
}

impl Daemon {
    #[cfg(test)]
    pub fn at(base: &str) -> Daemon {
        Daemon { base: base.into(), agent: ureq::AgentBuilder::new().timeout_connect(Duration::from_millis(300)).build(), token: None, socket: None, down: Default::default() }
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
            down: Default::default(),
        }
    }

    fn answer(&self, path: &str, r: Result<ureq::Response, ureq::Error>) -> CallResult {
        if !matches!(r, Err(ureq::Error::Transport(_))) {
            self.reached();
        }
        match r {
            // As the web board's `api()`: an OK answer that isn't JSON is just empty.
            Ok(resp) => Ok(resp.into_json::<Value>().unwrap_or(Value::Null)),
            Err(ureq::Error::Status(code, resp)) => {
                let msg = resp.into_json::<Value>().ok().and_then(|v| v["error"].as_str().map(str::to_string));
                Err(CallError { status: code, message: msg.unwrap_or_else(|| format!("The board answered {code}.")) })
            }
            Err(ureq::Error::Transport(t)) => {
                let mut why = t.kind().to_string();
                if let Some(m) = t.message() {
                    why += &format!(": {m}");
                }
                if let Some(s) = std::error::Error::source(&t) {
                    why += &format!(": {s}");
                }
                Err(self.unreachable(path, why))
            }
        }
    }

    /// The UI's "can't reach" error; the real reason goes to `app.log` when it's new.
    fn unreachable(&self, path: &str, why: String) -> CallError {
        let mut down = self.down.lock().unwrap_or_else(|e| e.into_inner());
        if down.as_deref() != Some(why.as_str()) {
            crate::log::info(format!("can't reach the board at {} ({path}): {why}", self.base));
            *down = Some(why);
        }
        crate::install::daemon_unreachable();
        CallError { status: 0, message: "Can’t reach the task board server.".into() }
    }

    fn reached(&self) {
        if self.down.lock().unwrap_or_else(|e| e.into_inner()).take().is_some() {
            crate::log::info(format!("reached the board at {} again", self.base));
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
        self.answer(path, req.call())
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
                Ok((status, answer)) => {
                    self.reached();
                    return socket_answer(status, &answer);
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => {}
                Err(e) => return Err(self.unreachable(path, format!("app socket {}: {e}", sock.display()))),
            }
        }
        let mut req = self.agent.post(&format!("{}/tasks/api/{path}", self.base)).set("X-Task-Board", "1").set("X-Task-Board-From", "app");
        if let Some(token) = token {
            req = req.set("X-Task-Board-Token", &token);
        }
        self.answer(path, req.send_json(body))
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
        seed_attachments(&board, &dir);
        if std::env::var("TASKBOARD_FAKE_RELEASES").is_ok() {
            seed_releases(&board);
        }
        Fake { board, dir }
    }
}

#[cfg(test)]
impl Fake {
    /// The board itself, for setting up what the API can't (a long log).
    pub fn app(&self) -> &Board {
        &self.board
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

/// The sign-in goal's files: its own design and doc, and screenshots, a video and results on its
/// tasks (the images are drawn into the board's folder so their previews show).
fn seed_attachments(board: &Board, dir: &std::path::Path) {
    use taskboardd::board::{OWNER, add_attachment};
    let shot = |name: &str, tone: [u8; 3]| -> String {
        let path = dir.join(name);
        let img = image::RgbImage::from_fn(720, 450, |x, y| {
            let band = if y < 54 { 0.55 } else if x < 160 { 0.85 } else { 1.0 };
            image::Rgb(tone.map(|c| (c as f32 * band) as u8))
        });
        let _ = img.save(&path);
        path.to_string_lossy().into_owned()
    };
    let task = |title: &str| -> Option<i64> {
        let row = board.db.q1("SELECT id FROM tasks WHERE title = ?", taskboardd::p![title]).ok()??;
        row["id"].as_i64()
    };
    let Some(goal) = board.db.q1("SELECT id FROM goals WHERE name = 'Faster sign-in'", vec![]).ok().flatten().and_then(|r| r["id"].as_i64()) else {
        return;
    };
    let on_goal: [(&str, &str, &str, &str); 2] = [
        ("https://claude.ai/artifact/sign-in-concepts", "Sign-in concepts", "design", ""),
        ("https://example.com/docs/sign-in-budget", "Latency budget", "doc", "Where each of the 900 ms goes."),
    ];
    for (url, title, kind, summary) in on_goal {
        let _ = add_attachment(board, OWNER, url, title, kind, summary, None, Some(goal));
    }
    let before = shot("sign-in-before.png", [214, 222, 236]);
    let after = shot("sign-in-after.png", [206, 232, 220]);
    let on_tasks = [
        ("Measure the sign-in path", before.as_str(), "Sign-in before", "evidence", "2.4 s from tap to home."),
        ("Measure the sign-in path", "https://example.com/traces/sign-in.csv", "Trace timings", "results", ""),
        ("Batch the session writes", after.as_str(), "Sign-in after", "evidence", "0.8 s with the writes batched."),
        ("Batch the session writes", "~/Movies/sign-in-walkthrough.mov", "Walkthrough", "evidence", ""),
        ("Write up the numbers", "https://example.com/docs/sign-in-numbers", "The numbers", "doc", ""),
    ];
    for (t, url, title, kind, summary) in on_tasks {
        if let Some(id) = task(t) {
            let _ = add_attachment(board, OWNER, url, title, kind, summary, Some(id), None);
        }
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

/// Dev (`TASKBOARD_FAKE_RELEASES=1`): releases on for both projects, two months of webapp releases
/// (one failed), work waiting in Unreleased and a pending cut, for the Releases pages.
fn seed_releases(board: &Board) {
    use taskboardd::board::update_task;
    use taskboardd::util::{iso, now_ts};
    let post = |path: &str, body: Value| api::dispatch(board, "POST", path, &Query::new(), &body).ok();
    let day = 86400.0;
    let goal = |name: &str| post("/goals", json!({"name": name, "project": "webapp"})).and_then(|g| g["id"].as_i64());
    let done = |title: &str, goal: Option<i64>, pr: Option<i64>, ago: f64| -> Option<i64> {
        let id = post("/tasks", json!({"title": title, "detail": "Sample.", "project": "webapp", "goal_id": goal, "pickup": {"mode": "manual"}}))?["id"].as_i64()?;
        let mut f = vec![("status", json!("done")), ("finished_at", json!(iso(now_ts() - ago)))];
        if let Some(n) = pr {
            f.extend([("pr_num", json!(n)), ("pr_url", json!(format!("https://github.com/acme/webapp/pull/{n}"))), ("pr_phase", json!("merged"))]);
        }
        update_task(board, id, f).ok()?;
        Some(id)
    };
    for p in ["webapp", "api"] {
        post(&format!("/projects/{p}"), json!({"releases": "on"}));
    }
    // The seed's own done task (T1) goes out in the oldest release.
    let speed = goal("Faster pages");
    let releases: [(&str, &str, f64, &[(&str, bool)]); 9] = [
        ("v0.9.0", "First public build", 58.0, &[("Ship the landing page", false), ("Add sign-up", true)]),
        ("v0.9.1", "Fix sign-up on Safari", 52.0, &[("Fix the Safari date input", false)]),
        ("v0.10.0", "Teams, invites", 44.0, &[("Invite a teammate by email", true), ("Team settings page", true), ("Seat limits", false)]),
        ("v0.10.1", "Invite link expiry", 37.0, &[("Expire invite links after a week", false)]),
        ("v0.11.0", "Faster dashboard", 21.0, &[("Cache the dashboard query", true), ("Lazy-load the charts", true)]),
        ("v0.11.1", "Hotfix for chart colors", 16.0, &[("Fix the chart palette in dark mode", false)]),
        ("v0.11.2", "Retry the color fix", 15.0, &[("Pin the palette tokens", false)]),
        ("v0.12.0", "Search, keyboard shortcuts", 6.0, &[("Search across projects", true), ("Keyboard shortcuts sheet", false), ("Remember the last filter", false)]),
        ("v0.13.0", "Passkey registration and faster first paint\nAlso trims the bundle.", 0.2, &[("Inline the critical CSS", true), ("Drop the unused icon font", true)]),
    ];
    let mut pr = 400;
    for (ix, (name, notes, ago, work)) in releases.iter().enumerate() {
        for (title, on_goal) in work.iter() {
            pr += 1;
            done(title, on_goal.then_some(speed).flatten(), Some(pr), ago + 0.5);
        }
        let Some(r) = post("/releases", json!({"name": name, "project": "webapp", "notes": notes, "no_script": true})) else { continue };
        let Some(id) = r["id"].as_i64() else { continue };
        let _ = board.db.update("releases", &json!(id), vec![("cut_at", json!(iso(now_ts() - ago * day)))]);
        if ix == 5 {
            post(&format!("/releases/{}/fail", r["ref"].as_str().unwrap_or("")), json!({"reason": "Notarizing timed out."}));
        }
    }
    // Unreleased work and a cut that waits for a running task and a goal still going.
    let next = goal("Releases page");
    done("Group releases by month", next, Some(431), 3600.0);
    done("Keep the newest release open", next, Some(432), 1800.0);
    done("Fix the ack button focus ring", None, Some(433), 900.0);
    let running = post("/tasks", json!({"title": "Link Releases from the phone app", "detail": "Sample.", "project": "webapp", "goal_id": next, "pickup": {"mode": "manual"}}))
        .and_then(|t| t["id"].as_i64());
    if let Some(t) = running {
        let _ = update_task(board, t, vec![("status", json!("working"))]);
    }
    let mut after: Vec<String> = vec!["T1".into()];
    after.extend(running.map(|t| format!("T{t}")));
    after.extend(next.map(|g| format!("G{g}")));
    post("/releases", json!({"name": "v0.14.0", "project": "webapp", "after": after, "no_script": true}));
    // api: one release and nothing waiting.
    let api_done = post("/tasks", json!({"title": "Rate-limit the login route", "detail": "Sample.", "project": "api", "pickup": {"mode": "manual"}})).and_then(|t| t["id"].as_i64());
    if let Some(t) = api_done {
        let _ = update_task(board, t, vec![("status", json!("done")), ("finished_at", json!(iso(now_ts() - 9.0 * day)))]);
        post("/releases", json!({"name": "v2.3.0", "project": "api", "no_script": true}));
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

    /// The UI keeps its sentence; the log gets why (refused, timed out…), once until it changes.
    #[::core::prelude::v1::test]
    fn unreachable_keeps_the_reason() {
        let d = Daemon::at("http://127.0.0.1:9");
        d.get("state", &[]).unwrap_err();
        let why = d.down.lock().unwrap().clone().expect("a reason");
        assert!(why.contains("refused"), "{why}");
        d.get("board", &[]).unwrap_err();
        assert_eq!(d.down.lock().unwrap().as_deref(), Some(why.as_str()));
        d.reached();
        assert!(d.down.lock().unwrap().is_none());
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
