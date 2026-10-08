//! The running board: config, database, log, and the signals its threads share.

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde_json::Value;

use crate::config::Config;
use crate::db::Db;
use crate::util::Result;

pub type Deferred = Box<dyn FnOnce(&App) + Send>;

#[derive(Default)]
pub struct Shared {
    pub midna_seen_at: Option<String>,
    pub midna_down: bool,
    pub midna_opened: Option<Instant>,
    pub midna_usage: Option<Value>,
    /// Midna's `agents.resume_after_network`, and when the board last read it.
    pub midna_resumes_network: Option<(Instant, bool)>,
    pub recent_reports: VecDeque<String>,
    pub recent_set: HashSet<String>,
    pub prs_checked_at: Option<String>,
    pub jira_token_off: bool,
    pub jira_creds: Option<(String, String)>,
    pub remotes: std::collections::HashMap<String, (Instant, Option<bool>)>,
}

struct Signal {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Signal {
    fn new() -> Self {
        Signal { flag: Mutex::new(false), cv: Condvar::new() }
    }
    fn set(&self) {
        *self.flag.lock() = true;
        self.cv.notify_all();
    }
    fn wait(&self, timeout: Duration) -> bool {
        let mut f = self.flag.lock();
        if !*f {
            self.cv.wait_for(&mut f, timeout);
        }
        std::mem::replace(&mut *f, false)
    }
}

pub struct App {
    pub cfg: Config,
    pub accounts: crate::accounts::Accounts,
    pub db: Db,
    pub shared: Mutex<Shared>,
    pub stopping: AtomicBool,
    log_file: Mutex<Option<std::fs::File>>,
    log_stderr: bool,
    runner_signal: Signal,
    jobs_signal: Signal,
    md_pending: Mutex<HashSet<i64>>,
    md_signal: Signal,
    deferred: Mutex<VecDeque<Deferred>>,
    deferred_signal: Signal,
    hook_jobs: Mutex<VecDeque<Deferred>>,
    hook_signal: Signal,
    pub inline_deferred: bool,
}

impl App {
    pub fn new(cfg: Config, log_stderr: bool) -> Result<Arc<App>> {
        std::fs::create_dir_all(&cfg.data).ok();
        std::fs::create_dir_all(cfg.md_dir()).ok();
        std::fs::create_dir_all(cfg.spool_dir()).ok();
        let db = Db::open(&cfg.db_path())?;
        let log_file = std::fs::OpenOptions::new().create(true).append(true).open(cfg.log_path()).ok();
        Ok(Arc::new(App::build(cfg, db, log_file, log_stderr, false)))
    }

    /// A board for tests: in-memory database, deferred work runs inline.
    pub fn for_tests(cfg: Config) -> Arc<App> {
        std::fs::create_dir_all(cfg.md_dir()).ok();
        std::fs::create_dir_all(cfg.spool_dir()).ok();
        Arc::new(App::build(cfg, Db::memory().unwrap(), None, false, true))
    }

    fn build(cfg: Config, db: Db, log_file: Option<std::fs::File>, log_stderr: bool, inline: bool) -> App {
        App {
            accounts: crate::accounts::Accounts::new(&cfg),
            cfg,
            db,
            shared: Mutex::new(Shared::default()),
            stopping: AtomicBool::new(false),
            log_file: Mutex::new(log_file),
            log_stderr,
            runner_signal: Signal::new(),
            jobs_signal: Signal::new(),
            md_pending: Mutex::new(HashSet::new()),
            md_signal: Signal::new(),
            deferred: Mutex::new(VecDeque::new()),
            deferred_signal: Signal::new(),
            hook_jobs: Mutex::new(VecDeque::new()),
            hook_signal: Signal::new(),
            inline_deferred: inline,
        }
    }

    pub fn info(&self, msg: impl AsRef<str>) {
        let line = format!("{} {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg.as_ref());
        if let Some(f) = self.log_file.lock().as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
        if self.log_stderr {
            eprint!("{line}");
        }
    }

    pub fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
        self.runner_signal.set();
        self.jobs_signal.set();
        self.md_signal.set();
        self.deferred_signal.set();
        self.hook_signal.set();
    }

    pub fn wake_runner(&self) {
        self.runner_signal.set();
    }

    pub fn wait_runner(&self, timeout: Duration) -> bool {
        self.runner_signal.wait(timeout)
    }

    pub fn notify_jobs(&self) {
        self.jobs_signal.set();
    }

    pub fn wait_jobs(&self, timeout: Duration) -> bool {
        self.jobs_signal.wait(timeout)
    }

    /// Sleeps up to `secs`, waking early when the board stops.
    pub fn sleep(&self, secs: f64) {
        let end = Instant::now() + Duration::from_secs_f64(secs.max(0.0));
        while !self.stopping() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(100).min(end - Instant::now()));
        }
    }

    pub fn schedule_md(&self, task_id: i64) {
        self.md_pending.lock().insert(task_id);
        self.md_signal.set();
    }

    pub fn take_md(&self, timeout: Duration) -> Vec<i64> {
        self.md_signal.wait(timeout);
        self.md_pending.lock().drain().collect()
    }

    pub fn defer(&self, f: Deferred) {
        if self.inline_deferred {
            f(self);
            return;
        }
        self.deferred.lock().push_back(f);
        self.deferred_signal.set();
    }

    pub fn next_deferred(&self, timeout: Duration) -> Option<Deferred> {
        if let Some(f) = self.deferred.lock().pop_front() {
            return Some(f);
        }
        self.deferred_signal.wait(timeout);
        self.deferred.lock().pop_front()
    }

    /// Owner hooks run one at a time on their own thread, in the order the board reached their steps.
    pub fn queue_hook(&self, f: Deferred) {
        if self.inline_deferred {
            f(self);
            return;
        }
        self.hook_jobs.lock().push_back(f);
        self.hook_signal.set();
    }

    pub fn next_hook(&self, timeout: Duration) -> Option<Deferred> {
        if let Some(f) = self.hook_jobs.lock().pop_front() {
            return Some(f);
        }
        self.hook_signal.wait(timeout);
        self.hook_jobs.lock().pop_front()
    }
}
