//! Time the Mac spends asleep doesn't count toward the board's waits. Its own test binary: the sleep
//! log is process-wide, and the flows in `flow.rs` mustn't see these sleeps.

use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{iso, now_ts, RowExt};
use taskboardd::{clock, jobs, p};

#[test]
fn a_job_running_across_a_sleep_doesnt_expire_on_wake() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let now = now_ts();
    let two_hours_ago = iso(now - 7200.0);
    let id = app
        .db
        .insert("jobs", taskboardd::fields!["kind" => "rename", "state" => "running", "args" => "{}", "created_at" => two_hours_ago, "updated_at" => two_hours_ago])
        .unwrap();
    let state = || app.db.q1("SELECT state FROM jobs WHERE id = ?", p![id]).unwrap().unwrap().st("state");

    clock::note_sleep(now - 7190.0, now);
    app.db.tx(|| jobs::expire(&app)).unwrap();
    assert_eq!(state(), "running", "only 10 awake seconds have passed");

    clock::forget_sleeps();
    app.db.tx(|| jobs::expire(&app)).unwrap();
    assert_eq!(state(), "expired", "two awake hours is too long");
}
