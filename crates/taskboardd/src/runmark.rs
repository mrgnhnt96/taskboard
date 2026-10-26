//! What the daemon leaves in server.log about how it lives and dies: a panic hook that writes the
//! thread, message, place and backtrace (launchd sends the daemon's stderr nowhere, so the default
//! hook's report is lost), and a run marker (`daemon.run` in the data folder) that says at the next
//! start how the last run ended: stopped cleanly and why, killed while stopping, or gone without a word.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::app::App;
use crate::config::Config;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn path(cfg: &Config) -> PathBuf {
    cfg.data.join("daemon.run")
}

/// A run as the marker records it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Mark {
    pub pid: u32,
    pub version: String,
    pub started: String,
    /// What made it stop, written as soon as the stop begins.
    pub stopping: Option<String>,
    /// When the stop finished; missing means the run ended before it could say so.
    pub stopped: Option<String>,
}

impl Mark {
    fn parse(text: &str) -> Option<Mark> {
        let mut m = Mark::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k {
                "pid" => m.pid = v.parse().unwrap_or(0),
                "version" => m.version = v.into(),
                "started" => m.started = v.into(),
                "stopping" => m.stopping = Some(v.into()),
                "stopped" => m.stopped = Some(v.into()),
                _ => {}
            }
        }
        (!m.version.is_empty()).then_some(m)
    }

    fn render(&self) -> String {
        let mut s = format!("pid={}\nversion={}\nstarted={}\n", self.pid, self.version, self.started);
        if let Some(r) = &self.stopping {
            s.push_str(&format!("stopping={}\n", r.replace('\n', " ")));
        }
        if let Some(t) = &self.stopped {
            s.push_str(&format!("stopped={t}\n"));
        }
        s
    }

    /// The startup line about how this (previous) run ended, seen from a run on `now`.
    pub fn describe(&self, now: &str) -> String {
        let who = format!("v{}, pid {}, started {}", self.version, self.pid, self.started);
        let mut s = match (&self.stopping, &self.stopped) {
            (Some(why), Some(at)) => format!("last run ({who}) stopped cleanly at {at}: {why}"),
            (Some(why), None) => format!("last run ({who}) began stopping ({why}) but didn't finish: it was killed while shutting down"),
            (None, _) => format!("last run ({who}) ended without a clean shutdown: it crashed, was killed (SIGKILL) or the Mac went down; look above for a panic"),
        };
        if self.version != now {
            s.push_str(&format!("; now v{now}"));
        }
        s
    }
}

fn read(p: &Path) -> Option<Mark> {
    Mark::parse(&std::fs::read_to_string(p).ok()?)
}

fn write(app: &App, m: &Mark) {
    let p = path(&app.cfg);
    let tmp = p.with_extension("run.tmp");
    if let Err(e) = std::fs::write(&tmp, m.render()).and_then(|_| std::fs::rename(&tmp, &p)) {
        app.info(format!("can't write {}: {e}; the next start won't know how this run ended", p.display()));
    }
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// At startup: logs how the last run ended and marks this one running.
pub fn begin(app: &App) {
    if let Some(prev) = read(&path(&app.cfg)) {
        app.info(prev.describe(VERSION));
    }
    write(app, &Mark { pid: std::process::id(), version: VERSION.into(), started: now(), ..Default::default() });
}

fn update(app: &App, f: impl FnOnce(&mut Mark)) {
    let mut m = read(&path(&app.cfg))
        .filter(|m| m.pid == std::process::id())
        .unwrap_or_else(|| Mark { pid: std::process::id(), version: VERSION.into(), ..Default::default() });
    f(&mut m);
    write(app, &m);
}

/// The stop has begun, for `reason` (a signal, the server failing).
pub fn stopping(app: &App, reason: &str) {
    app.info(format!("stopping: {reason}"));
    update(app, |m| m.stopping = Some(reason.into()));
}

/// The stop finished; the next start reads this run as a clean one.
pub fn stopped(app: &App) {
    app.info(format!("taskboardd {VERSION} stopped"));
    update(app, |m| {
        m.stopping.get_or_insert_with(|| "the server stopped".into());
        m.stopped = Some(now());
    });
}

fn payload_text(p: &(dyn std::any::Any + Send)) -> &str {
    p.downcast_ref::<&str>().copied().or_else(|| p.downcast_ref::<String>().map(String::as_str)).unwrap_or("(no message)")
}

/// Sends every panic to server.log: the thread, the message, where, and a backtrace. A panic in one
/// of the board's threads ends that thread and its work, so this is the only trace it leaves.
pub fn install_panic_hook(app: &Arc<App>) {
    let log = Arc::downgrade(app);
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let at = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_else(|| "an unknown place".into());
        let bt = std::backtrace::Backtrace::force_capture();
        let line = format!("panic in thread '{name}' at {at}: {}\n{bt}", payload_text(info.payload()));
        match log.upgrade() {
            Some(a) => a.info(line),
            None => eprintln!("{line}"),
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark() -> Mark {
        Mark { pid: 42, version: "1.0.0-beta.63".into(), started: "2026-10-24 09:00:00".into(), ..Default::default() }
    }

    #[test]
    fn round_trips_through_the_file_format() {
        let mut m = mark();
        m.stopping = Some("SIGTERM\nfrom launchd".into());
        m.stopped = Some("2026-10-24 10:00:00".into());
        let back = Mark::parse(&m.render()).unwrap();
        assert_eq!(back.stopping.as_deref(), Some("SIGTERM from launchd"));
        assert_eq!(back.stopped, m.stopped);
        assert_eq!(back.pid, 42);
        assert!(Mark::parse("").is_none());
    }

    #[test]
    fn says_how_the_last_run_ended() {
        let mut m = mark();
        let gone = m.describe("1.0.0-beta.63");
        assert!(gone.contains("ended without a clean shutdown"), "{gone}");
        assert!(!gone.contains("now v"), "{gone}");
        m.stopping = Some("SIGTERM".into());
        assert!(m.describe("1.0.0-beta.63").contains("began stopping (SIGTERM) but didn't finish"));
        m.stopped = Some("2026-10-24 10:00:00".into());
        let clean = m.describe("1.0.0-beta.64");
        assert!(clean.contains("stopped cleanly at 2026-10-24 10:00:00: SIGTERM"), "{clean}");
        assert!(clean.ends_with("; now v1.0.0-beta.64"), "{clean}");
    }

    #[test]
    fn marks_a_run_from_start_to_clean_stop() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::for_tests(Config::for_tests(dir.path()));
        begin(&app);
        let running = read(&path(&app.cfg)).unwrap();
        assert_eq!(running.pid, std::process::id());
        assert!(running.stopping.is_none() && running.stopped.is_none());
        stopping(&app, "ctrl-c");
        stopped(&app);
        let done = read(&path(&app.cfg)).unwrap();
        assert_eq!(done.stopping.as_deref(), Some("ctrl-c"));
        assert!(done.stopped.is_some());
        assert_eq!(done.started, running.started);
    }
}
