//! The app's own log, `app.log` beside the daemon's `server.log` in the board's data folder, in
//! the same `<date> <time> <message>` lines. An app opened from Finder has no terminal, so what it
//! only printed was lost. Every line still goes to stderr too, for `cargo run`. It rolls over like
//! `server.log` (`taskboardd::logfile`).
use std::sync::Mutex;

use taskboardd::logfile::Log;

static FILE: Mutex<Option<Log>> = Mutex::new(None);

/// Open `app.log` and log panics there. The sample board (`TASKBOARD_BACKEND=fake`) keeps to stderr,
/// so dev runs and snapshots leave the real log alone.
pub fn init() {
    if std::env::var("TASKBOARD_BACKEND").as_deref() != Ok("fake") {
        let data = taskboardd::config::Config::load().map(|c| c.data).unwrap_or_else(|_| taskboardd::config::default_data_dir());
        std::fs::create_dir_all(&data).ok();
        *FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Log::open(data.join("app.log"), taskboardd::logfile::CAP));
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |p| {
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let msg = p.payload().downcast_ref::<&str>().copied().or_else(|| p.payload().downcast_ref::<String>().map(String::as_str)).unwrap_or("(no message)");
        let at = p.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        write(&format!("panic on thread {thread}{at}: {msg}\n{}", std::backtrace::Backtrace::force_capture()));
        prev(p);
    }));
}

/// One line in `app.log`, and on stderr.
pub fn info(msg: impl AsRef<str>) {
    eprintln!("taskboard-app: {}", msg.as_ref());
    write(msg.as_ref());
}

fn write(msg: &str) {
    let line = format!("{} {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg.trim_end());
    if let Some(f) = FILE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        f.write(&line);
    }
}
