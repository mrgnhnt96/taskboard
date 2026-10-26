//! The running board: config, database, log, and the signals its threads share.

use std::collections::{HashMap, HashSet, VecDeque};
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
    /// Each usage window's last known limits (`usage.rs`), so a report without one doesn't blank its meter.
    pub usage_known: serde_json::Map<String, Value>,
    /// Midna's `keep_awake.status` from the last sync (absent on a Midna without keep-awake).
    pub midna_keep_awake: Option<Value>,
    /// The schedule the board last gave Midna's keep-awake, and when.
    pub keep_awake_pushed: Option<(Instant, Value)>,
    /// Midna's `agents.resume_after_network`, and when the board last read it.
    pub midna_resumes_network: Option<(Instant, bool)>,
    /// When the board last checked its link patterns in Midna, and the Midna (pid and start) that has them.
    pub midna_links: Option<(Instant, Option<String>)>,
    /// The Midna run that has the board's link menus and the menus it was given (empty when that
    /// Midna doesn't take menus).
    pub midna_menus: Option<(String, String)>,
    /// The Midna run (pid and start) rid of the tab-status triggers older boards kept there.
    pub midna_old_triggers: Option<String>,
    /// Tab statuses waiting to go to Midna: each terminal, what its tab shows and what it's waiting
    /// on, if anything (`midna::set_tab`).
    pub midna_tabs: VecDeque<(String, &'static str, Option<String>)>,
    /// Needs-you tab statuses sent outside the work hours without their needs_you base, by terminal,
    /// to put it back once they open (`midna::release_tabs`).
    pub midna_held_tabs: HashMap<String, (&'static str, Option<String>)>,
    pub recent_reports: VecDeque<String>,
    pub recent_set: HashSet<String>,
    pub prs_checked_at: Option<String>,
    pub jira_token_off: bool,
    pub jira_creds: Option<(String, String)>,
    pub remotes: std::collections::HashMap<String, (Instant, Option<bool>)>,
    /// Each repo's `git config user.email`, and when it was read (`picker::selves`).
    pub git_emails: std::collections::HashMap<String, (Instant, Option<String>)>,
    /// `projects::list_projects` as of a `Db::mark`: every task card asks for its project's path.
    pub projects: Option<((i64, i64), Vec<serde_json::Value>)>,
    /// Claude is grouping new backlog issues; and when it last failed (it waits before trying again).
    pub grouping: bool,
    pub group_failed_at: Option<Instant>,
    /// Claude is tidying the backlog (`tidy.rs`).
    pub tidying: bool,
    /// The ideas Claude is digging into now (`dig.rs`).
    pub digging: HashSet<i64>,
    /// The ideas Claude is planning into goals now (`idea_plan.rs`).
    pub idea_planning: HashSet<i64>,
    /// The latest wave plan asked for on the Backlog page (`GET /backlog/plan`).
    pub plan: Option<serde_json::Value>,
    pub plan_seq: i64,
    /// QA comments being read by Claude right now (`qa.rs`).
    pub qa_reading: HashSet<i64>,
    /// The last `tb pr …` read of each task's PR, shared with callers that asked while it ran (`prcmds`).
    pub pr_reads: HashMap<i64, Arc<crate::prcmds::Flight>>,
    /// PR hosts used in place of the real ones (`prhost::install`, tests).
    pub pr_hosts: std::collections::HashMap<String, Arc<dyn crate::prhost::PrHost>>,
    /// Default-branch CI readers used in place of the real ones (`breaks::install`, tests).
    pub branch_ci: std::collections::HashMap<String, Arc<dyn crate::breaks::BranchCi>>,
    /// Alerts' notification responses Midna reported (alert id, `notify.response`), for the next tick.
    pub notify_inbox: Arc<Mutex<Vec<(String, Value)>>>,
    /// Alerts with a thread waiting on their notification's response: alert id -> the `notified_at` of its
    /// latest notification (`dispatch::Waiter`).
    pub alert_waiters: Arc<Mutex<std::collections::HashMap<String, String>>>,
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
    ai_jobs: Mutex<VecDeque<Deferred>>,
    ai_signal: Signal,
    lens_jobs: Mutex<VecDeque<Deferred>>,
    lens_signal: Signal,
    pub inline_deferred: bool,
    /// This launch's app token (`apptoken`): what makes a request the app's own.
    pub app_token: String,
    /// Who this launch takes for the app (`apporigin`); set by `serve`, else the unsigned default.
    pub app_origin: once_cell::sync::OnceCell<crate::apporigin::Trust>,
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
            ai_jobs: Mutex::new(VecDeque::new()),
            ai_signal: Signal::new(),
            lens_jobs: Mutex::new(VecDeque::new()),
            lens_signal: Signal::new(),
            inline_deferred: inline,
            app_token: crate::apptoken::fresh(),
            app_origin: once_cell::sync::OnceCell::new(),
        }
    }

    /// Who this launch takes for the app (`apporigin::Trust`).
    pub fn trust(&self) -> &crate::apporigin::Trust {
        self.app_origin.get_or_init(|| crate::apporigin::Trust::unsigned(self.cfg.app_origin.token))
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
        self.ai_signal.set();
        self.lens_signal.set();
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

    /// Headless Claude calls (backlog grouping and wave plans) run one at a time on their own
    /// thread: each takes a minute or more and mustn't hold up other work.
    pub fn queue_ai(&self, f: Deferred) {
        if self.inline_deferred {
            f(self);
            return;
        }
        self.ai_jobs.lock().push_back(f);
        self.ai_signal.set();
    }

    pub fn next_ai(&self, timeout: Duration) -> Option<Deferred> {
        if let Some(f) = self.ai_jobs.lock().pop_front() {
            return Some(f);
        }
        self.ai_signal.wait(timeout);
        self.ai_jobs.lock().pop_front()
    }

    /// Lens reviews of new PRs (`lens_pr.rs`) and idea digs (`dig.rs`) run one at a time on their own
    /// thread: each takes about 15 minutes, too long to wait behind or hold up the other headless calls.
    pub fn queue_lens(&self, f: Deferred) {
        if self.inline_deferred {
            f(self);
            return;
        }
        self.lens_jobs.lock().push_back(f);
        self.lens_signal.set();
    }

    pub fn next_lens(&self, timeout: Duration) -> Option<Deferred> {
        if let Some(f) = self.lens_jobs.lock().pop_front() {
            return Some(f);
        }
        self.lens_signal.wait(timeout);
        self.lens_jobs.lock().pop_front()
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
