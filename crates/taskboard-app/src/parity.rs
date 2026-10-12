//! Parity with the web board this app replaced (tests only).
//!
//! `parity/web/` is a frozen copy of the old web UI. `parity/gen/*.mjs` run its own functions on
//! fixtures and write the answers to `parity/golden/<area>.json` (`parity/regen.sh`, needs node).
//! The tests here and in each page module feed the same inputs to the app's functions and expect
//! the same answers, so a regression against the web behaviour fails `cargo test`.
//!
//! Goldens use a frozen clock (`now` in the file) and UTC; [`freeze`] sets the same for the app.
//!
//! Also here: [`Recording`], a backend that runs the real board in-process (like `Fake`) and
//! records every POST, and [`window`], which opens a headless `MainWindow` on it so tests can
//! drive the UI's actions and assert the exact requests the web board would have sent.
#![allow(dead_code)] // helpers for the page modules' tests

use crate::backend::{Backend, CallResult, Fake};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::cell::Cell;
use std::sync::{Arc, Mutex};

thread_local! {
    static NOW: Cell<Option<DateTime<Utc>>> = const { Cell::new(None) };
}

pub fn frozen_now() -> Option<DateTime<Utc>> {
    NOW.with(|n| n.get())
}

/// Freeze `fmt::now()` on this thread (tests run each on their own thread).
pub fn freeze(iso: &str) {
    let t = DateTime::parse_from_rfc3339(iso).expect("frozen time").with_timezone(&Utc);
    NOW.with(|n| n.set(Some(t)));
}

pub struct Golden {
    pub cases: Vec<Value>,
}

/// `parity/golden/<name>.json`, with the clock frozen at its `now`.
pub fn golden(name: &str) -> Golden {
    let path = format!("{}/parity/golden/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e} (run parity/regen.sh)"));
    let v: Value = serde_json::from_str(&text).expect("golden json");
    freeze(v["now"].as_str().expect("golden now"));
    Golden { cases: v["cases"].as_array().cloned().unwrap_or_default() }
}

impl Golden {
    /// Run `f` on every case's input and collect mismatches, so one run reports all of them.
    pub fn check(&self, mut f: impl FnMut(&Value) -> Value) {
        let mut bad = Vec::new();
        for c in &self.cases {
            let got = f(&c["input"]);
            if got != c["expect"] {
                bad.push(format!("  {}: web {} / app {}", c["name"].as_str().unwrap_or("?"), c["expect"], got));
            }
        }
        assert!(bad.is_empty(), "{} of {} cases differ from the web board:\n{}", bad.len(), self.cases.len(), bad.join("\n"));
    }

    /// Only the cases whose `input.fn` is `name`.
    pub fn only(&self, name: &str) -> Golden {
        Golden { cases: self.cases.iter().filter(|c| c["input"]["fn"] == name).cloned().collect() }
    }
}

// ------------------------------------------------------------------ recording backend

/// The in-process sample board, recording every POST (path, body) in order.
pub struct Recording {
    inner: Fake,
    pub posts: Mutex<Vec<(String, Value)>>,
    /// GET paths that fail with this (status, sentence) until cleared.
    fail: Mutex<Vec<(String, u16, String)>>,
}

impl Recording {
    pub fn new() -> Arc<Recording> {
        Arc::new(Recording { inner: Fake::new(), posts: Mutex::new(Vec::new()), fail: Mutex::new(Vec::new()) })
    }

    pub fn posts(&self) -> Vec<(String, Value)> {
        self.posts.lock().unwrap().clone()
    }

    /// The last POST to `path`.
    pub fn last(&self, path: &str) -> Option<Value> {
        self.posts.lock().unwrap().iter().rev().find(|(p, _)| p == path).map(|(_, b)| b.clone())
    }

    pub fn clear(&self) {
        self.posts.lock().unwrap().clear();
    }

    /// Make `GET <path>` fail with the board's `status` and sentence (empty `msg`: unreachable).
    pub fn fail(&self, path: &str, status: u16, msg: &str) {
        self.fail.lock().unwrap().push((path.to_string(), status, msg.to_string()));
    }

    pub fn heal(&self) {
        self.fail.lock().unwrap().clear();
    }

    /// Direct access for setting up a case (GET or POST without recording).
    pub fn board(&self) -> &Fake {
        &self.inner
    }
}

impl Backend for Recording {
    fn label(&self) -> &'static str {
        "recording"
    }
    fn get(&self, path: &str, query: &[(&str, String)]) -> CallResult {
        if let Some((_, status, msg)) = self.fail.lock().unwrap().iter().find(|(p, _, _)| p == path) {
            return Err(crate::backend::CallError { status: *status, message: msg.clone() });
        }
        self.inner.get(path, query)
    }
    fn post(&self, path: &str, body: Value) -> CallResult {
        self.posts.lock().unwrap().push((path.to_string(), body.clone()));
        self.inner.post(path, body)
    }
}

// ------------------------------------------------------------------ headless window

/// A `MainWindow` on a fresh [`Recording`] board, after its first refresh landed.
pub fn window(cx: &mut gpui_kit::TestAppContext) -> (gpui_kit::WindowHandle<crate::app::MainWindow>, Arc<Recording>) {
    let rec = Recording::new();
    crate::prefs::reset();
    cx.update(|cx| {
        cx.set_global(crate::theme::Theme::new(crate::theme::ThemeMode::Light, "Helvetica Neue".into(), "Menlo".into()));
        crate::app::bind_keys(cx);
    });
    let be: Arc<dyn Backend> = rec.clone();
    let w = cx.add_window(|window, cx| crate::app::MainWindow::new(be, window, cx));
    settle(cx);
    (w, rec)
}

/// Let spawned refreshes and posts finish (the backend is synchronous, so this terminates).
pub fn settle(cx: &mut gpui_kit::TestAppContext) {
    for _ in 0..3 {
        cx.run_until_parked();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmt;
    use serde_json::json;

    #[::core::prelude::v1::test]
    fn fmt_matches_web() {
        let g = golden("fmt");
        g.check(|i| match i["fn"].as_str().unwrap() {
            "ago" => json!(fmt::ago(i["iso"].as_str().unwrap())),
            "span" => json!(fmt::span(i["m"].as_i64().unwrap())),
            "clock12" => json!(fmt::clock12(i["hhmm"].as_str().unwrap())),
            "whenLine" => json!(fmt::when_line(&i["task"])),
            "ref" => json!(fmt::ref_of(&i["x"], i["p"].as_str().unwrap())),
            f => panic!("no app function for {f}"),
        });
    }

    #[gpui_kit::test]
    fn headless_window_loads_the_board(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |m, _, _| {
            assert!(m.data.state.is_some(), "state fetched");
            assert!(!m.data.goals.is_empty(), "goals fetched");
        })
        .unwrap();
        assert!(rec.posts().is_empty(), "loading the board posts nothing");
    }

    #[gpui_kit::test]
    fn notification_links_open_the_right_place(cx: &mut gpui_kit::TestAppContext) {
        use crate::app::{Page, Panel};
        let (w, _rec) = window(cx);
        w.update(cx, |m, _, cx| m.open_link("taskboard://task/t3", cx)).unwrap();
        settle(cx);
        w.update(cx, |m, _, _| {
            assert!(matches!(&m.panel, Some(Panel::Task { r, .. }) if r == "T3"));
            assert_eq!(m.data.task.as_ref().and_then(|t| t["ref"].as_str()), Some("T3"), "task detail fetched");
        })
        .unwrap();
        w.update(cx, |m, _, cx| m.open_link("taskboard-dev://goal/G1", cx)).unwrap();
        settle(cx);
        w.update(cx, |m, _, _| assert_eq!(m.page, Page::Goal("G1".into()))).unwrap();
        w.update(cx, |m, _, cx| m.open_link("taskboard://session/fake-s2", cx)).unwrap();
        settle(cx);
        w.update(cx, |m, _, _| {
            assert_eq!(m.page, Page::Sessions);
            assert_eq!(m.sessions.selected.as_deref(), Some("fake-s2"));
        })
        .unwrap();
        w.update(cx, |m, _, cx| m.open_link("taskboard://", cx)).unwrap();
        w.update(cx, |m, _, _| assert_eq!(m.page, Page::Board)).unwrap();
    }

    /// `loadTask`: a failed fetch keeps the panel open with the board's sentence in place of
    /// "Loading…" (`S.taskErr`), and the next good answer clears it.
    #[gpui_kit::test]
    fn a_missing_task_shows_the_boards_sentence(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m, _, cx| m.open_task("T99", cx)).unwrap();
        settle(cx);
        w.update(cx, |m, _, _| {
            assert!(m.panel.is_some(), "the panel stays open, as on the web");
            assert!(m.data.task.is_none());
            assert_eq!(m.data.errs.task.as_deref(), Some("There's no task T99."), "the board's own sentence");
        })
        .unwrap();
    }

    /// Lists keep what they showed when a refresh fails, remember why, and clear it on the
    /// next good answer (`SS.listErr`, `SS.detailErr`, `goalErr`, `issueErr`).
    #[gpui_kit::test]
    fn failed_fetches_keep_the_page_and_say_why(cx: &mut gpui_kit::TestAppContext) {
        use crate::app::Page;
        let (w, rec) = window(cx);
        // Sessions: list and detail.
        rec.fail("sessions", 500, "The board hit an error. It's in the board's log.");
        rec.fail("sessions/fake-s1", 404, "There's nothing at that address.");
        w.update(cx, |m, _, cx| {
            m.sessions.selected = Some("fake-s1".into());
            m.go(Page::Sessions, cx);
        })
        .unwrap();
        settle(cx);
        w.update(cx, |m, _, _| {
            assert_eq!(m.data.errs.sessions.as_deref(), Some("The board hit an error. It's in the board's log."));
            assert_eq!(m.data.errs.session.as_deref(), Some("There's nothing at that address."));
            assert!(m.data.session.is_none(), "a failed detail for another terminal shows nothing");
        })
        .unwrap();
        // Goal page.
        rec.fail("goals/G1", 500, "Boom.");
        w.update(cx, |m, _, cx| m.go(Page::Goal("G1".into()), cx)).unwrap();
        settle(cx);
        w.update(cx, |m, _, _| assert_eq!(m.data.errs.goal.as_deref(), Some("Boom."))).unwrap();
        // While the board is down, only the banner speaks.
        rec.heal();
        rec.fail("state", 0, "Can’t reach the task board server.");
        rec.fail("goals/G1", 500, "should not be asked");
        w.update(cx, |m, _, cx| m.refresh(cx)).unwrap();
        settle(cx);
        w.update(cx, |m, _, _| {
            assert_eq!(m.down.as_deref(), Some("Can’t reach the task board server."));
            assert_eq!(m.data.errs.goal.as_deref(), Some("Boom."), "unchanged: nothing else was fetched");
        })
        .unwrap();
    }

    /// Switching goals starts the page empty (`resetPageState`), not on the old goal.
    #[gpui_kit::test]
    fn another_goal_starts_empty(cx: &mut gpui_kit::TestAppContext) {
        use crate::app::Page;
        let (w, _rec) = window(cx);
        w.update(cx, |m, _, cx| m.go(Page::Goal("G1".into()), cx)).unwrap();
        settle(cx);
        w.update(cx, |m, _, _| assert!(m.data.goal.is_some())).unwrap();
        w.update(cx, |m, _, cx| {
            m.go(Page::Goal("G9".into()), cx);
            assert!(m.data.goal.is_none(), "G1 isn't shown as G9 while it loads");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn filters_persist_like_the_web(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m, _, cx| {
            let f = crate::app::Filters { project: "webapp".into(), goal: "G1".into(), done: "7d".into() };
            m.set_filters(f, cx);
        })
        .unwrap();
        let saved = crate::prefs::get("taskboard.filter").unwrap();
        assert_eq!(saved, json!({"project": "webapp", "goal": "G1", "done": "7d"}));
        let f = crate::app::Filters::load();
        assert_eq!((f.project.as_str(), f.goal.as_str(), f.done.as_str()), ("webapp", "G1", "7d"));
        // Invalid saved values fall back field by field (loadFilter).
        crate::prefs::set("taskboard.filter", json!({"project": 3, "goal": "nope", "done": "1y"}));
        let f = crate::app::Filters::load();
        assert_eq!((f.project.as_str(), f.goal.as_str(), f.done.as_str()), ("all", "all", "24h"));
    }

    /// The task panel's "N open in the goal’s backlog" opens that goal on its Backlog tab.
    #[gpui_kit::test]
    fn goal_backlog_link_opens_the_backlog_tab(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = window(cx);
        w.update(cx, |m, _, cx| crate::ui::goal::open(m, "G1", true, cx)).unwrap();
        settle(cx);
        w.update(cx, |m, _, _| {
            assert_eq!(m.page, crate::app::Page::Goal("G1".into()));
            assert!(m.goal_page.backlog_view);
        })
        .unwrap();
    }
}
