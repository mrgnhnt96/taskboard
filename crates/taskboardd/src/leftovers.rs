//! What a task's agent starts and leaves running, and cleaning it up once the task is done with it.
//!
//! Once a minute the board looks at the owner's processes for the kinds agents leave behind: Chrome
//! driven over a debugging port (ultrahand, parity checkers), Gradle and Kotlin daemons, Android
//! Studio, and the pool's simulators and emulators that are booted. Each one it sees is recorded
//! against the task it belongs to:
//! - a process started from a task's terminal carries that terminal's `MIDNA_SESSION`, and belongs to
//!   the task the terminal was on when the process started (`task_terminals`);
//! - Chrome and Node rewrite their own environment, so a Chrome the board first sees within a few
//!   minutes of its start belongs to the task holding the worktree slot it was started in (its cwd);
//! - a pool device belongs to the task it's lent to;
//! - a booted simulator outside the pool belongs to no task: no task can lease it.
//!
//! A recorded process is cleaned up when its task stops being active (done, failed, parked; a done
//! task a PR visit is on still counts), or when the terminal it was started from is gone while the
//! task carries on in another. A Chrome is sent SIGTERM (SIGKILL if it's still there `KILL_AFTER`
//! later) and its profile removed when it's under the temp dir; a device is shut down with its own
//! stop command (`xcrun simctl shutdown` for a simulator outside the pool). Gradle and Kotlin daemons wait while any Gradle build or Android Studio is running,
//! since another task's build may be using them. What a task's terminal left behind once it's gone
//! (anything else carrying its `MIDNA_SESSION`, outside an app bundle and not in `KEEP`) goes too.
//!
//! Simulators are leased like devices: one no task has leased (in the pool or not) is shut down once
//! it's been up `[cleanup] unleased_simulator_mins`, and at once while more simulators are booted than
//! the pool has, so no more than the pool's simulators stay up. Nothing shows who booted a simulator
//! (launchd starts it), so one the owner booted by hand goes too.
//!
//! The orphan sweep: a recorded Chrome or emulator with no task, or one from a terminal that's gone,
//! is cleaned up once it's older than `[cleanup] orphan_hours`. A Gradle or Kotlin daemon or Android
//! Studio with no task is never touched: it may be the owner's own. Nothing outside an agent's reach
//! is touched either: other users' processes, the board itself and what started it, apps started
//! with `open`, and terminals the board doesn't know as a task's.
//!
//! Each task's log says what was stopped for it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;

use crate::app::App;
use crate::util::*;
use crate::{board, p};

pub const SCHEMA: &str = r#"
-- What an agent started and left running (`leftovers.rs`): a process by its pid and start time,
-- the task (and terminal) it belongs to, and when it was stopped and found gone.
CREATE TABLE IF NOT EXISTS leftovers(
  id INTEGER PRIMARY KEY, kind TEXT NOT NULL, pid INT NOT NULL, started INT NOT NULL,
  task_id INT, session_id TEXT, device TEXT, profile TEXT, what TEXT,
  seen_at TEXT, term_at TEXT, cleaned_at TEXT, how TEXT, UNIQUE(pid, started));
CREATE INDEX IF NOT EXISTS leftovers_open ON leftovers(cleaned_at);
"#;

/// Columns added since the first schema: a booted device's UDID or serial, for one outside the pool.
pub const ADDED: &[(&str, &str, &str)] = &[("leftovers", "target", "TEXT")];

/// `[cleanup]` in config.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CleanupConfig {
    /// Stops what a task's agent left running once the task is done with it.
    pub on: bool,
    /// How old a Chrome or booted emulator with no live task gets before the sweep stops it.
    pub orphan_hours: f64,
    /// How long a simulator no task has leased stays up before it's shut down.
    pub unleased_simulator_mins: f64,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        CleanupConfig { on: true, orphan_hours: 12.0, unleased_simulator_mins: 10.0 }
    }
}

/// Seconds between passes.
pub const EVERY: f64 = 60.0;
/// A Chrome seen this soon after its start belongs to the task holding the slot it runs in.
const FRESH: i64 = 300;
/// A terminal gone this long, with nothing of it still attached to Midna, has really ended.
const GONE_GRACE: f64 = 300.0;
/// Seconds after SIGTERM before SIGKILL.
const KILL_AFTER: f64 = 10.0;
/// Seconds a device's stop command may take.
const STOP_TIMEOUT: f64 = 60.0;

/// What a gone terminal leaves running that's never stopped: shared services, and the board and
/// Midna themselves.
const KEEP: &[&str] = &[
    "taskboardd", "taskboard-app", "tb", "midna", "midnad", "sccache", "limactl", "colima", "docker", "com.docker.backend",
    "ssh-agent", "gpg-agent", "launchd",
];

const CHROMES: &[&str] = &[
    "Google Chrome", "Google Chrome for Testing", "Google Chrome Canary", "Chromium", "chrome", "chrome-headless-shell",
    "headless_shell",
];

/// One of the owner's processes: its arguments and, when it kept it, its terminal's `MIDNA_SESSION`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Proc {
    pub pid: i64,
    pub ppid: i64,
    /// Unix seconds.
    pub started: i64,
    pub exe: String,
    pub args: Vec<String>,
    pub session: Option<String>,
    pub cwd: Option<String>,
}

impl Proc {
    fn has(&self, s: &str) -> bool {
        self.args.iter().any(|a| a.contains(s))
    }

    /// `--name=value` or `--name value`.
    fn flag(&self, name: &str) -> Option<String> {
        let eq = format!("{name}=");
        self.args.iter().enumerate().find_map(|(i, a)| {
            if let Some(v) = a.strip_prefix(&eq) {
                return Some(v.to_string());
            }
            (a == name).then(|| self.args.get(i + 1).cloned()).flatten()
        })
    }

    fn base(&self) -> &str {
        let exe = if self.exe.is_empty() { self.args.first().map(String::as_str).unwrap_or("") } else { &self.exe };
        exe.rsplit('/').next().unwrap_or(exe)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Chrome,
    Gradle,
    Kotlin,
    Studio,
    Simulator,
    Emulator,
    /// Anything else a gone terminal left running.
    Left,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Chrome => "chrome",
            Kind::Gradle => "gradle",
            Kind::Kotlin => "kotlin",
            Kind::Studio => "studio",
            Kind::Simulator => "simulator",
            Kind::Emulator => "emulator",
            Kind::Left => "left",
        }
    }

    fn from_name(s: &str) -> Option<Kind> {
        [Kind::Chrome, Kind::Gradle, Kind::Kotlin, Kind::Studio, Kind::Simulator, Kind::Emulator, Kind::Left]
            .into_iter()
            .find(|k| k.name() == s)
    }

    fn device(self) -> bool {
        matches!(self, Kind::Simulator | Kind::Emulator)
    }

    fn daemon(self) -> bool {
        matches!(self, Kind::Gradle | Kind::Kotlin)
    }
}

/// The kinds agents leave behind; None for anything else.
pub fn kind_of(p: &Proc) -> Option<Kind> {
    if p.has("org.gradle.launcher.daemon.bootstrap.GradleDaemon") {
        return Some(Kind::Gradle);
    }
    if p.has("org.jetbrains.kotlin.daemon.KotlinCompileDaemon") {
        return Some(Kind::Kotlin);
    }
    if p.exe.contains("Android Studio") && p.exe.contains(".app/Contents/MacOS/") {
        return Some(Kind::Studio);
    }
    let base = p.base();
    if base == "launchd_sim" {
        return Some(Kind::Simulator);
    }
    if base.starts_with("qemu-system") {
        return Some(Kind::Emulator);
    }
    let main = !p.args.iter().any(|a| a.starts_with("--type="));
    let driven = p.args.iter().any(|a| a.starts_with("--remote-debugging-port") || a.starts_with("--remote-debugging-pipe"));
    (CHROMES.contains(&base) && main && driven).then_some(Kind::Chrome)
}

static UDID: Lazy<Regex> = Lazy::new(|| Regex::new(r"/Devices/([0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12})/").unwrap());
static SIM_ID: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}$").unwrap());

/// A pool device that's a simulator: its target is a simulator UDID (a phone's id isn't one).
fn pool_simulator(d: &Row) -> bool {
    SIM_ID.is_match(&d.st("target").trim().to_uppercase())
}

/// The id a booted simulator or emulator goes by in the pool: its UDID, or `emulator-<port>`.
pub fn device_target(p: &Proc, kind: Kind) -> Option<String> {
    match kind {
        Kind::Simulator => p.args.iter().find_map(|a| UDID.captures(a).map(|c| c[1].to_uppercase())),
        Kind::Emulator => p.flag("-port").map(|port| format!("emulator-{port}")),
        _ => None,
    }
}

fn describe(p: &Proc, kind: Kind, device: Option<&str>, target: Option<&str>) -> String {
    match kind {
        Kind::Chrome => match p.flag("--remote-debugging-port") {
            Some(port) => format!("Chrome on port {port}"),
            None => "Chrome".into(),
        },
        Kind::Gradle => "a Gradle daemon".into(),
        Kind::Kotlin => "a Kotlin daemon".into(),
        Kind::Studio => "Android Studio".into(),
        Kind::Simulator | Kind::Emulator => match (device, target) {
            (Some(d), _) => d.to_string(),
            (None, Some(t)) => format!("simulator {t}"),
            (None, None) => "a device".into(),
        },
        Kind::Left => format!("{} (pid {})", p.base(), p.pid),
    }
}

/// What the pass does to the Mac; the tests record it instead.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Term(i64),
    Kill(i64),
    /// A device's stop command, for `sh -c`.
    Stop(String),
    Remove(PathBuf),
}

/// What the board knows that decides whose a process is and whether it's still wanted.
struct Board {
    now: f64,
    orphan_secs: f64,
    unleased_secs: f64,
    /// More simulators are booted than the pool has.
    too_many_sims: bool,
    /// Terminal → (when a task took it, the task), oldest first.
    terms: HashMap<String, Vec<(f64, i64)>>,
    /// Terminal → when the board saw it go.
    gone: HashMap<String, f64>,
    /// Terminals with a process still under Midna (not detached to launchd).
    attached: HashSet<String>,
    /// Slot path → the task holding it.
    slots: Vec<(String, i64)>,
    /// Pool device target → its row.
    devices: HashMap<String, Row>,
    /// Device name → the active task it's lent to.
    lent: HashMap<String, i64>,
    live: HashMap<i64, bool>,
}

impl Board {
    fn load(app: &App, procs: &[Proc]) -> Result<Board> {
        let mut terms: HashMap<String, Vec<(f64, i64)>> = HashMap::new();
        for r in app.db.q("SELECT task_id, session_id, at FROM task_terminals ORDER BY at", p![])? {
            terms.entry(r.st("session_id")).or_default().push((r.s("at").and_then(parse_iso).unwrap_or(0.0), r.i0("task_id")));
        }
        let gone = app
            .db
            .q("SELECT id, gone_at FROM sessions WHERE gone_at IS NOT NULL", p![])?
            .into_iter()
            .filter_map(|r| Some((r.st("id"), parse_iso(r.s("gone_at")?)?)))
            .collect();
        let by_pid: HashMap<i64, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
        let attached = procs
            .iter()
            .filter(|p| p.ppid > 1)
            .filter_map(|p| {
                let s = p.session.as_ref()?;
                (by_pid.get(&p.ppid).and_then(|q| q.session.as_ref()) != Some(s)).then(|| s.clone())
            })
            .collect();
        let slots = app
            .db
            .q("SELECT path, task_id FROM worktree_slots WHERE task_id IS NOT NULL", p![])?
            .into_iter()
            .map(|r| (r.st("path"), r.i0("task_id")))
            .collect();
        let devices: HashMap<String, Row> = app
            .db
            .q("SELECT * FROM devices WHERE COALESCE(target, '') != ''", p![])?
            .into_iter()
            .map(|d| (d.st("target").to_uppercase(), d))
            .collect();
        let lent = app
            .db
            .q(
                &format!(
                    "SELECT l.device, l.task_id FROM device_loans l JOIN tasks t ON t.id = l.task_id WHERE l.released_at IS NULL AND {}",
                    crate::devices::ACTIVE_SQL
                ),
                p![],
            )?
            .into_iter()
            .map(|r| (r.st("device"), r.i0("task_id")))
            .collect();
        let pool_sims = devices.values().filter(|d| !d.b("off") && pool_simulator(d)).count();
        let booted_sims = procs.iter().filter(|p| kind_of(p) == Some(Kind::Simulator) && device_target(p, Kind::Simulator).is_some()).count();
        Ok(Board {
            now: now_ts(),
            orphan_secs: app.cfg.cleanup.orphan_hours.max(0.0) * 3600.0,
            unleased_secs: app.cfg.cleanup.unleased_simulator_mins.max(0.0) * 60.0,
            too_many_sims: booted_sims > pool_sims,
            terms,
            gone,
            attached,
            slots,
            devices,
            lent,
            live: HashMap::new(),
        })
    }

    /// The task that was on terminal `s` at `at`.
    fn task_on(&self, s: &str, at: f64) -> Option<i64> {
        self.terms.get(s)?.iter().rev().find(|(from, _)| *from <= at + 5.0).map(|(_, t)| *t)
    }

    fn gone(&self, s: &str) -> bool {
        self.gone.get(s).is_some_and(|at| self.now - at >= GONE_GRACE) && !self.attached.contains(s)
    }

    fn slot_holder(&self, cwd: &str) -> Option<i64> {
        self.slots.iter().find(|(path, _)| cwd == path || cwd.starts_with(&format!("{path}/"))).map(|(_, t)| *t)
    }

    /// Still working: active, or done with a PR visit on it.
    fn live(&mut self, app: &App, id: i64) -> bool {
        if let Some(l) = self.live.get(&id) {
            return *l;
        }
        let sql = format!("SELECT COUNT(*) FROM tasks t WHERE t.id = ? AND {}", crate::devices::ACTIVE_SQL);
        let l = app.db.count(&sql, p![id]).unwrap_or(0) > 0
            || board::get_task(app, id).is_ok_and(|t| crate::prflow::visitor(&t).is_some());
        self.live.insert(id, l);
        l
    }

    /// Whose a process just seen is: the task and the terminal it was started from.
    fn owner(&self, p: &Proc, kind: Kind, device: Option<&Row>) -> (Option<i64>, Option<String>) {
        if kind.device() {
            return (device.and_then(|d| self.lent.get(&d.st("name")).copied()), None);
        }
        if let Some(s) = &p.session {
            return (self.task_on(s, p.started as f64), Some(s.clone()));
        }
        let fresh = self.now as i64 - p.started <= FRESH;
        (p.cwd.as_deref().filter(|_| fresh).and_then(|c| self.slot_holder(c)), None)
    }
}

/// Whether a recorded process should go now.
fn due(app: &App, b: &mut Board, r: &Row, kind: Kind) -> bool {
    let up = b.now - r.i0("started") as f64;
    let old = up >= b.orphan_secs;
    if kind.device() {
        if b.lent.contains_key(&r.st("device")) {
            return false;
        }
        if r.i("task_id").is_some() {
            return true;
        }
        return match kind {
            Kind::Simulator => b.too_many_sims || up >= b.unleased_secs,
            _ => old,
        };
    }
    let session = r.s("session_id").map(str::to_string);
    let gone = session.as_deref().is_some_and(|s| b.gone(s));
    match r.i("task_id") {
        Some(t) => !b.live(app, t) || gone,
        None if session.is_none() && kind != Kind::Chrome => false,
        None => (session.is_none() || gone) && old,
    }
}

/// A Chrome profile the board may delete: one under the temp dir, never the temp dir itself.
fn temp_profile(path: &str) -> bool {
    let p = Path::new(path);
    let mut roots = vec![std::env::temp_dir(), PathBuf::from("/tmp"), PathBuf::from("/private/tmp")];
    if let Ok(c) = std::env::temp_dir().canonicalize() {
        roots.push(c);
    }
    !path.contains("/../") && roots.iter().any(|r| p.starts_with(r) && p != r.as_path())
}

/// The pids never to signal: this process and the ones that started it.
fn ours(procs: &[Proc]) -> HashSet<i64> {
    let by_pid: HashMap<i64, i64> = procs.iter().map(|p| (p.pid, p.ppid)).collect();
    let mut out = HashSet::from([0, 1]);
    let mut pid = std::process::id() as i64;
    while pid > 1 && out.insert(pid) {
        pid = by_pid.get(&pid).copied().unwrap_or(1);
    }
    out
}

/// One pass over `procs` (the owner's processes now): records what's new, stops what's due through
/// `act`, and marks what's gone. Returns how many it stopped.
pub fn pass(app: &App, procs: &[Proc], act: &mut dyn FnMut(&Act) -> bool) -> Result<usize> {
    let mut b = Board::load(app, procs)?;
    let now = now_iso();
    let keep = ours(procs);
    let by_key: HashMap<(i64, i64), &Proc> = procs.iter().map(|p| ((p.pid, p.started), p)).collect();
    let known: HashSet<(i64, i64)> = app
        .db
        .q("SELECT pid, started FROM leftovers", p![])?
        .iter()
        .map(|r| (r.i0("pid"), r.i0("started")))
        .collect();

    for p in procs.iter().filter(|p| !keep.contains(&p.pid) && !known.contains(&(p.pid, p.started))) {
        let kind = match kind_of(p) {
            Some(k) => k,
            None => {
                let Some(s) = &p.session else { continue };
                let base = p.base();
                if p.exe.contains(".app/") || KEEP.contains(&base) || !b.gone(s) {
                    continue;
                }
                Kind::Left
            }
        };
        let target = device_target(p, kind);
        let device = match &target {
            Some(t) => match b.devices.get(t) {
                Some(d) => Some(d.clone()),
                None if kind == Kind::Simulator => None,
                None => continue,
            },
            None if kind.device() => continue,
            None => None,
        };
        let (task, session) = b.owner(p, kind, device.as_ref());
        if kind == Kind::Left && task.is_none() {
            continue;
        }
        let profile = (kind == Kind::Chrome).then(|| p.flag("--user-data-dir")).flatten();
        app.db.insert(
            "leftovers",
            crate::fields![
                "kind" => kind.name(), "pid" => p.pid, "started" => p.started, "task_id" => task, "session_id" => session,
                "device" => device.as_ref().map(|d| d.st("name")), "target" => target, "profile" => profile,
                "what" => describe(p, kind, device.as_ref().map(|d| d.st("name")).as_deref(), target.as_deref()), "seen_at" => now
            ],
        )?;
    }

    // A Gradle or Kotlin daemon may be serving another task's build: wait while one runs.
    let building = procs.iter().any(|p| {
        p.has("org.gradle.wrapper.GradleWrapperMain") || p.has("org.gradle.launcher.GradleMain") || kind_of(p) == Some(Kind::Studio)
    });
    let mut stopped: HashMap<Option<i64>, Vec<String>> = HashMap::new();
    for r in app.db.q("SELECT * FROM leftovers WHERE cleaned_at IS NULL ORDER BY id", p![])? {
        let Some(kind) = Kind::from_name(r.s("kind").unwrap_or("")) else { continue };
        let alive = by_key.contains_key(&(r.i0("pid"), r.i0("started")));
        if !alive {
            if let Some(path) = r.s("profile").filter(|p| temp_profile(p) && Path::new(p).exists()) {
                act(&Act::Remove(PathBuf::from(path)));
            }
            let how = if r.s("term_at").is_some() { "stopped" } else { "exited" };
            app.db.x("UPDATE leftovers SET cleaned_at = ?, how = ? WHERE id = ?", p![now, how, r.id()])?;
            continue;
        }
        if kind.device() {
            if let Some(t) = b.lent.get(&r.st("device")).copied().filter(|t| r.i("task_id") != Some(*t)) {
                app.db.x("UPDATE leftovers SET task_id = ? WHERE id = ?", p![t, r.id()])?;
                continue;
            }
        }
        if let Some(at) = r.s("term_at").and_then(parse_iso) {
            if kind.device() {
                if b.now - at >= STOP_TIMEOUT * 2.0 {
                    app.info(format!("cleanup: {} didn't shut down", r.st("what")));
                    app.db.x("UPDATE leftovers SET cleaned_at = ?, how = 'still running' WHERE id = ?", p![now, r.id()])?;
                }
            } else if b.now - at >= KILL_AFTER {
                act(&Act::Kill(r.i0("pid")));
            }
            continue;
        }
        if !due(app, &mut b, &r, kind) || (kind.daemon() && building) || keep.contains(&r.i0("pid")) {
            continue;
        }
        let done = if kind.device() {
            let pool = b.devices.values().find(|d| r.s("device").is_some_and(|n| d.st("name") == n)).cloned();
            let cmd = match (pool, r.s("target")) {
                (Some(d), _) => stop_cmd(app, &d, r.i("task_id")),
                (None, Some(t)) if kind == Kind::Simulator && SIM_ID.is_match(t) => format!("xcrun simctl shutdown {t}"),
                _ => continue,
            };
            act(&Act::Stop(cmd))
        } else {
            act(&Act::Term(r.i0("pid")))
        };
        if done {
            app.db.x("UPDATE leftovers SET term_at = ? WHERE id = ?", p![now, r.id()])?;
            stopped.entry(r.i("task_id")).or_default().push(r.st("what"));
        }
    }

    let mut n = 0;
    for (task, what) in stopped {
        n += what.len();
        let line = format!("Stopped what was left running: {}", what.join(", "));
        match task {
            Some(t) => app.db.tx(|| board::log_event(app, t, board::BOARD, "note", &line).map(|_| ()))?,
            None => app.info(format!("cleanup: {} (no task left to own them)", line.to_lowercase())),
        }
    }
    Ok(n)
}

/// A device's stop command filled for the task it was lent to, else with its own fields only.
fn stop_cmd(app: &App, d: &Row, task: Option<i64>) -> String {
    let given = d.s("stop_cmd").map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
    let cmd = given.unwrap_or_else(|| match d.st("target").starts_with("emulator-") {
        true => "adb -s {target} emu kill".into(),
        false => "xcrun simctl shutdown {target}".into(),
    });
    let vars = match task.and_then(|t| board::get_task(app, t).ok()) {
        Some(t) => crate::devices::cmd_vars(app, &t, d),
        None => ["device", "name", "kind", "target"]
            .into_iter()
            .map(|k| (k.to_string(), if k == "device" { d.st("name") } else { d.st(k) }))
            .collect(),
    };
    crate::steps::fill_shell(&cmd, &vars)
}

/// Carries out an `Act` on this Mac.
pub fn perform(app: &App, a: &Act) -> bool {
    match a {
        Act::Term(pid) => unsafe { libc::kill(*pid as libc::pid_t, libc::SIGTERM) == 0 },
        Act::Kill(pid) => unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL) == 0 },
        Act::Remove(path) => std::fs::remove_dir_all(path).is_ok(),
        Act::Stop(cmd) => match crate::proc::run(Path::new("/bin/sh"), &["-c".into(), cmd.clone()], None, STOP_TIMEOUT) {
            Ok(o) if o.code == Some(0) => true,
            Ok(o) => {
                app.info(format!("cleanup: `{cmd}` failed: {}", o.stderr.trim()));
                true
            }
            Err(_) => false,
        },
    }
}

/// The cleanup thread: a pass a minute while `[cleanup] on`.
pub fn run(app: std::sync::Arc<App>) {
    while !app.stopping() {
        if app.cfg.cleanup.on {
            let procs = scan();
            if let Err(e) = pass(&app, &procs, &mut |a| perform(&app, a)) {
                app.info(format!("cleanup failed: {}", e.message));
            }
        }
        app.sleep(EVERY);
    }
}

/// Splits `KERN_PROCARGS2` output: the executable's path, its arguments and its environment.
pub fn parse_procargs(b: &[u8]) -> Option<(String, Vec<String>, Vec<String>)> {
    let argc = i32::from_ne_bytes(b.get(..4)?.try_into().ok()?).max(0) as usize;
    let rest = &b[4..];
    let end = rest.iter().position(|&c| c == 0)?;
    let exe = String::from_utf8_lossy(&rest[..end]).into_owned();
    let mut words = rest[end..].split(|&c| c == 0).skip_while(|w| w.is_empty());
    let args: Vec<String> = words.by_ref().take(argc).map(|w| String::from_utf8_lossy(w).into_owned()).collect();
    let env = words.take_while(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned()).collect();
    Some((exe, args, env))
}

/// The owner's processes now, with the cwd of each Chrome found.
#[cfg(target_os = "macos")]
pub fn scan() -> Vec<Proc> {
    use std::mem::{size_of, zeroed};
    use std::os::raw::{c_int, c_void};

    let uid = unsafe { libc::getuid() };
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let mut pids = vec![0 as c_int; n.max(0) as usize + 256];
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * size_of::<c_int>()) as c_int) };
    pids.truncate(n.max(0) as usize);

    let mut argmax: c_int = 0;
    let mut len = size_of::<c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    unsafe { libc::sysctl(mib.as_mut_ptr(), 2, &mut argmax as *mut _ as *mut c_void, &mut len, std::ptr::null_mut(), 0) };
    let mut buf = vec![0u8; argmax.clamp(4096, 4 << 20) as usize];

    let mut out = vec![];
    for pid in pids.into_iter().filter(|&p| p > 0) {
        let mut info: libc::proc_bsdinfo = unsafe { zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as c_int;
        if unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size) } != size
            || info.pbi_uid != uid
        {
            continue;
        }
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut len = buf.len();
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut c_void, &mut len, std::ptr::null_mut(), 0) } != 0 {
            continue;
        }
        let Some((exe, args, env)) = parse_procargs(&buf[..len]) else { continue };
        let session = env.iter().find_map(|e| e.strip_prefix("MIDNA_SESSION=")).filter(|s| !s.is_empty()).map(str::to_string);
        let mut p = Proc { pid: pid as i64, ppid: info.pbi_ppid as i64, started: info.pbi_start_tvsec as i64, exe, args, session, cwd: None };
        if kind_of(&p) == Some(Kind::Chrome) {
            p.cwd = cwd(pid);
        }
        out.push(p);
    }
    out
}

#[cfg(target_os = "macos")]
fn cwd(pid: i32) -> Option<String> {
    use std::mem::{size_of, zeroed};
    let mut info: libc::proc_vnodepathinfo = unsafe { zeroed() };
    let size = size_of::<libc::proc_vnodepathinfo>() as i32;
    if unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, &mut info as *mut _ as *mut std::os::raw::c_void, size) } != size {
        return None;
    }
    let raw = &info.pvi_cdir.vip_path;
    let bytes = unsafe { std::slice::from_raw_parts(raw.as_ptr() as *const u8, size_of::<[[libc::c_char; 32]; 32]>()) };
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    (end > 0).then(|| String::from_utf8_lossy(&bytes[..end]).into_owned())
}

#[cfg(not(target_os = "macos"))]
pub fn scan() -> Vec<Proc> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(exe: &str, args: &[&str]) -> Proc {
        Proc { pid: 10, ppid: 1, started: 0, exe: exe.into(), args: args.iter().map(|a| a.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn knows_the_kinds_agents_leave_behind() {
        let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        let driven = proc(chrome, &[chrome, "--remote-debugging-port=9742", "--user-data-dir=/var/folders/x/T/ultrahand-drive-a"]);
        assert_eq!(kind_of(&driven), Some(Kind::Chrome));
        assert_eq!(driven.flag("--user-data-dir").as_deref(), Some("/var/folders/x/T/ultrahand-drive-a"));
        assert_eq!(describe(&driven, Kind::Chrome, None, None), "Chrome on port 9742");
        assert_eq!(kind_of(&proc(chrome, &[chrome])), None, "the owner's own Chrome");
        assert_eq!(kind_of(&proc(chrome, &[chrome, "--type=renderer", "--remote-debugging-port=1"])), None, "a helper goes with its main");

        let java = "/Applications/Android Studio.app/Contents/jbr/Contents/Home/bin/java";
        assert_eq!(kind_of(&proc(java, &[java, "-cp", "x", "org.gradle.launcher.daemon.bootstrap.GradleDaemon", "9.3.1"])), Some(Kind::Gradle));
        assert_eq!(kind_of(&proc(java, &[java, "org.jetbrains.kotlin.daemon.KotlinCompileDaemon"])), Some(Kind::Kotlin));
        assert_eq!(kind_of(&proc("/Applications/Android Studio.app/Contents/MacOS/studio", &[])), Some(Kind::Studio));
        assert_eq!(kind_of(&proc("/usr/bin/zsh", &["zsh"])), None);
    }

    #[test]
    fn reads_a_booted_devices_pool_id() {
        let sim = proc(
            "/Library/Developer/PrivateFrameworks/CoreSimulator.framework/Resources/bin/launchd_sim",
            &["launchd_sim", "/Users/m/Library/Developer/CoreSimulator/Devices/10c58c88-f29d-4b4b-9160-338b32728665/data/var/run/launchd_bootstrap.plist"],
        );
        assert_eq!(kind_of(&sim), Some(Kind::Simulator));
        assert_eq!(device_target(&sim, Kind::Simulator).as_deref(), Some("10C58C88-F29D-4B4B-9160-338B32728665"));
        let emu = proc("/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64", &["qemu-system-aarch64", "-avd", "x", "-port", "5570"]);
        assert_eq!(device_target(&emu, Kind::Emulator).as_deref(), Some("emulator-5570"));
    }

    #[test]
    fn splits_procargs_into_path_arguments_and_environment() {
        let mut b = 2i32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"/bin/sleep\0\0\0\0sleep\x0020\0MIDNA_SESSION=ab12\0HOME=/h\0\0junk");
        let (exe, args, env) = parse_procargs(&b).unwrap();
        assert_eq!(exe, "/bin/sleep");
        assert_eq!(args, vec!["sleep", "20"]);
        assert_eq!(env, vec!["MIDNA_SESSION=ab12", "HOME=/h"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn scan_sees_this_process() {
        let me = std::process::id() as i64;
        let p = scan().into_iter().find(|p| p.pid == me).expect("this test's own process");
        assert!(p.started > 0 && p.ppid > 0);
        assert!(p.exe.ends_with(&p.base().to_string()) && !p.args.is_empty());
    }

    #[test]
    fn deletes_only_profiles_under_the_temp_dir() {
        let tmp = std::env::temp_dir();
        assert!(temp_profile(&tmp.join("ultrahand-drive-x").to_string_lossy()));
        assert!(!temp_profile(&tmp.to_string_lossy()));
        assert!(!temp_profile("/Users/m/Library/Application Support/Google/Chrome"));
        assert!(!temp_profile("/tmp/../Users/m"));
    }
}
