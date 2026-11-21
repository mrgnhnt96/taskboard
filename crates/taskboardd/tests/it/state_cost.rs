//! What `GET /state` costs on a board of realistic size: thousands of done tasks, and a goal whose
//! waves hold many queued tasks. Counted in database reads rather than time, so a busy Mac can't fail
//! it and a fast one can't hide a regression.

use std::sync::Arc;

use serde_json::json;
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::fields;
use taskboardd::util::{iso, now_iso, now_ts};

const DONE: usize = 3000;

/// A board with `DONE` tasks finished last month and one goal of `queued` queued tasks in three waves,
/// the first wave holding one task still working.
fn board(queued: usize) -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let old = iso(now_ts() - 30.0 * 86400.0);
    app.db
        .tx(|| {
            for i in 0..DONE {
                app.db.insert("tasks", fields!["title" => format!("Done {i}"), "project" => "webapp", "status" => "done",
                                               "created_at" => old.clone(), "updated_at" => old.clone(), "finished_at" => old.clone()])?;
            }
            let g = app.db.insert("goals", fields!["name" => "Big plan", "project" => "webapp", "max_terminals" => 4])?;
            app.db.insert("tasks", fields!["title" => "First", "project" => "webapp", "status" => "working", "goal_id" => g,
                                           "wave" => 1, "position" => 0, "created_at" => now_iso()])?;
            for i in 0..queued {
                app.db.insert("tasks", fields!["title" => format!("Queued {i}"), "project" => "webapp", "status" => "queued",
                                               "goal_id" => g, "wave" => 1 + (i % 3) as i64, "position" => i as i64 + 1,
                                               "created_at" => now_iso()])?;
            }
            Ok(())
        })
        .unwrap();
    (app, dir)
}

/// The database reads one `GET /state` makes, built afresh.
fn state_reads(app: &App) -> (u64, usize) {
    app.shared.lock().state_boards.clear();
    let before = app.db.reads();
    let v = api::dispatch(app, "GET", "state", &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET state: {}", e.message));
    let queued = v["columns"]["queued"].as_array().map(|a| a.len()).unwrap_or(0);
    (app.db.reads() - before, queued)
}

#[test]
fn state_reads_grow_with_the_queued_cards_not_their_square() {
    let (small, _d1) = board(20);
    let (big, _d2) = board(80);
    let (small_reads, small_cards) = state_reads(&small);
    let (big_reads, big_cards) = state_reads(&big);
    assert_eq!((small_cards, big_cards), (20, 80));
    // Each queued card used to ask its goal for its waves again, and each wave asks every task in it for
    // its locks, slot and lane: four times the cards cost fourteen times the reads (937 a card). Read once
    // a request, a card costs about 34.
    let per_card = (big_reads - small_reads) / 60;
    assert!(
        big_reads < small_reads * 5 && per_card <= PER_CARD,
        "20 queued cards: {small_reads} reads; 80: {big_reads} ({per_card} per card)"
    );
}

/// The reads one more queued card in a waved goal may add.
const PER_CARD: u64 = 50;

#[test]
fn a_repeat_state_inside_the_cache_window_reads_almost_nothing() {
    let (app, _dir) = board(40);
    let (first, _) = state_reads(&app);
    let before = app.db.reads();
    api::dispatch(&app, "GET", "state", &Query::new(), &json!({})).unwrap();
    let again = app.db.reads() - before;
    assert!(again * 10 < first, "a fresh build: {first} reads; a kept one: {again}");
}
