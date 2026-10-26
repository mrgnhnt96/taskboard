//! taskboard: a local task board that runs Claude Code sessions in the Midna terminal app and tracks their work.

pub mod accounts;
pub mod adf;
pub mod areas;
pub mod ask;
pub mod api;
pub mod app;
pub mod apporigin;
pub mod apptoken;
pub mod asks;
pub mod bits;
pub mod board;
pub mod botrun;
pub mod breaks;
pub mod bring;
pub mod builds;
pub mod chain;
pub mod checkin;
pub mod clock;
pub mod comments;
pub mod config;
pub mod days;
pub mod design;
pub mod db;
pub mod deliver;
pub mod devfocus;
pub mod devices;
pub mod dig;
pub mod digest;
pub mod dispatch;
pub mod export;
pub mod feed;
pub mod gitattrs;
pub mod handoff;
pub mod hooks;
pub mod home;
pub mod hours;
pub mod hunks;
pub mod human;
pub mod import;
pub mod cutover;
pub mod jira;
pub mod jira_claude;
pub mod jira_desk;
pub mod jira_status;
pub mod jobs;
pub mod keep_awake;
pub mod lanes;
pub mod lens;
pub mod lens_pr;
pub mod lens_sweep;
pub mod ideas;
pub mod idea_plan;
pub mod idea_talk;
pub mod lessons;
pub mod limits;
pub mod lines;
pub mod lost_done;
pub mod locks;
pub mod logfile;
pub mod mdcopy;
pub mod merged_note;
pub mod midna;
pub mod ops;
pub mod picker;
pub mod planning;
pub mod prcmds;
pub mod presence;
pub mod prbar;
pub mod prbuilds;
pub mod prflow;
pub mod prhost;
pub mod prpace;
pub mod proc;
pub mod progress;
pub mod palette;
pub mod projects;
pub mod propen;
pub mod qa;
pub mod quality;
pub mod pending_cut;
pub mod release_run;
pub mod release_tags;
pub mod releases;
pub mod relevance;
pub mod reproject;
pub mod reqcache;
pub mod reports;
pub mod retro;
pub mod reviewers;
pub mod runner;
pub mod runmark;
pub mod screen;
pub mod seed;
pub mod server;
pub mod setup;
pub mod shared;
pub mod review;
pub mod review_fixes;
pub mod slot_ageing;
pub mod slot_tidy;
pub mod slots;
pub mod slowlog;
pub mod spans;
pub mod tidy;
pub mod stack;
pub mod suggest;
pub mod startword;
pub mod statusbar;
pub mod stream;
pub mod steps;
pub mod transcript;
pub mod triage;
pub mod usage;
pub mod util;
pub mod waitsfor;
pub mod waves;
pub mod worklane;
pub mod worktrees;


use std::sync::Arc;
use std::time::Duration;

use app::App;

/// Starts a named board thread on `f`, logging when the thread can't be made (its work would
/// otherwise just never run).
fn spawn(app: &Arc<App>, name: &str, f: impl FnOnce(Arc<App>) + Send + 'static) {
    let a = app.clone();
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(move || f(a)) {
        app.info(format!("can't start the {name} thread: {e}; its work won't run until the daemon restarts"));
    }
}

/// Starts the board's background threads: the md writer, deferred work, Midna sync and, with the
/// runner on, Midna jobs and the runner.
pub fn start_threads(app: &Arc<App>) {
    spawn(app, "clock", clock::watch);
    spawn(app, "md", mdcopy::writer_loop);
    spawn(app, "deferred", |a| {
        while !a.stopping() {
            if let Some(f) = a.next_deferred(Duration::from_secs(5)) {
                f(&a);
            }
        }
    });
    spawn(app, "hooks", |a| {
        while !a.stopping() {
            if let Some(f) = a.next_hook(Duration::from_secs(5)) {
                f(&a);
            }
        }
    });
    spawn(app, "reports", |a| {
        while !a.stopping() {
            a.take_reports(Some(Duration::from_secs(5)));
        }
    });
    spawn(app, "lens", |a| {
        while !a.stopping() {
            if let Some(f) = a.next_lens(Duration::from_secs(5)) {
                f(&a);
            }
        }
    });
    spawn(app, "history", days::cleanup_loop);
    spawn(app, "backlog-ai", |a| {
        while !a.stopping() {
            if let Some(f) = a.next_ai(Duration::from_secs(5)) {
                f(&a);
            }
        }
    });
    spawn(app, "midna-sync", midna::sync_loop);
    if app.cfg.runner {
        spawn(app, "midna-jobs", midna::jobs_loop);
        spawn(app, "runner", runner::run);
        spawn(app, "feed-listener", feed::listener_loop);
        spawn(app, "checkin", checkin::run);
        spawn(app, "suggest", suggest::run);
        spawn(app, "spans", spans::run);
        spawn(app, "review", review::run);
        spawn(app, "lens-sweep", lens_sweep::run);
    } else {
        spawn(app, "spool", |a| {
            while !a.stopping() {
                let _ = reports::ingest_spool(&a);
                a.sleep(a.cfg.intervals.spool);
            }
        });
    }
}
