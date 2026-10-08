//! taskboard: a local task board that runs Claude Code sessions in the Midna terminal app and tracks their work.

pub mod accounts;
pub mod api;
pub mod app;
pub mod board;
pub mod clock;
pub mod config;
pub mod db;
pub mod deliver;
pub mod dispatch;
pub mod handoff;
pub mod hours;
pub mod jira;
pub mod jobs;
pub mod mdcopy;
pub mod midna;
pub mod ops;
pub mod prflow;
pub mod proc;
pub mod projects;
pub mod reports;
pub mod runner;
pub mod screen;
pub mod seed;
pub mod server;
pub mod transcript;
pub mod usage;
pub mod util;
pub mod waitsfor;


use std::sync::Arc;
use std::time::Duration;

use app::App;

/// Starts the board's background threads: the md writer, deferred work, Midna sync and, with the
/// runner on, Midna jobs and the runner.
pub fn start_threads(app: &Arc<App>) {
    let a = app.clone();
    std::thread::Builder::new().name("clock".into()).spawn(move || clock::watch(a)).ok();
    let a = app.clone();
    std::thread::Builder::new().name("md".into()).spawn(move || mdcopy::writer_loop(a)).ok();
    let a = app.clone();
    std::thread::Builder::new()
        .name("deferred".into())
        .spawn(move || {
            while !a.stopping() {
                if let Some(f) = a.next_deferred(Duration::from_secs(5)) {
                    f(&a);
                }
            }
        })
        .ok();
    let a = app.clone();
    std::thread::Builder::new().name("midna-sync".into()).spawn(move || midna::sync_loop(a)).ok();
    if app.cfg.runner {
        let a = app.clone();
        std::thread::Builder::new().name("midna-jobs".into()).spawn(move || midna::jobs_loop(a)).ok();
        let a = app.clone();
        std::thread::Builder::new().name("runner".into()).spawn(move || runner::run(a)).ok();
    } else {
        let a = app.clone();
        std::thread::Builder::new()
            .name("spool".into())
            .spawn(move || {
                while !a.stopping() {
                    let _ = reports::ingest_spool(&a);
                    a.sleep(a.cfg.intervals.spool);
                }
            })
            .ok();
    }
}
